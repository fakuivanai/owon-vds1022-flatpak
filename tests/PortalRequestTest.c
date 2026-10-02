/* Real D-Bus request scenarios for the documented subscribe-before-call rule:
 * https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Request.html
 * This test runs its own bus and fake portal. It never calls the desktop portal.
 */
#include "portal.h"

typedef enum {
    EARLY_RESPONSE,
    LATE_RESPONSE,
    CANCELLED_RESPONSE,
    FAILED_RESPONSE,
    WRONG_PATH,
    MALFORMED_RESPONSE,
    BUS_DISCONNECT,
    SERVICE_DISCONNECT
} Scenario;

typedef struct {
    Scenario scenario;
    GTestDBus *test_bus;
    GMainContext *context;
    GMainLoop *loop;
    GDBusConnection *bus;
    GMutex lock;
    GCond condition;
    gboolean ready;
    gchar *request_path;
} MockPortal;

static const char *const PORTAL_NAME = "org.freedesktop.portal.Desktop";
static const char *const PORTAL_PATH = "/org/freedesktop/portal/desktop";
static const char *const INTERFACE = "org.freedesktop.portal.FileChooser";
static const char *const XML =
    "<node><interface name='org.freedesktop.portal.FileChooser'>"
    "<method name='SaveFile'>"
    "<arg name='parent' type='s' direction='in'/>"
    "<arg name='title' type='s' direction='in'/>"
    "<arg name='options' type='a{sv}' direction='in'/>"
    "<arg name='request' type='o' direction='out'/>"
    "</method></interface></node>";

static void emit_response(MockPortal *portal)
{
    GVariantBuilder results;
    g_variant_builder_init(&results, G_VARIANT_TYPE_VARDICT);
    g_variant_builder_add(&results, "{sv}", "test-value", g_variant_new_uint32(29));
    GVariant *parameters;
    if (portal->scenario == MALFORMED_RESPONSE) {
        parameters = g_variant_new("(s@a{sv})", "invalid-status", g_variant_builder_end(&results));
    } else {
        guint response = portal->scenario == CANCELLED_RESPONSE ? 1
            : portal->scenario == FAILED_RESPONSE ? 2 : 0;
        parameters = g_variant_new("(u@a{sv})", response, g_variant_builder_end(&results));
    }
    g_autoptr(GError) error = NULL;
    gboolean emitted = g_dbus_connection_emit_signal(
        portal->bus, NULL, portal->request_path, "org.freedesktop.portal.Request",
        "Response", parameters, &error);
    g_assert_no_error(error);
    g_assert_true(emitted);
}

static gboolean emit_later(gpointer data)
{
    emit_response(data);
    return G_SOURCE_REMOVE;
}

static gboolean disconnect_later(gpointer data)
{
    MockPortal *portal = data;
    if (portal->scenario == BUS_DISCONNECT) {
        g_test_dbus_stop(portal->test_bus);
    } else {
        g_autoptr(GError) error = NULL;
        g_assert_true(g_dbus_connection_close_sync(portal->bus, NULL, &error));
        g_assert_no_error(error);
    }
    return G_SOURCE_REMOVE;
}

static void schedule(MockPortal *portal, GSourceFunc callback)
{
    GSource *source = g_timeout_source_new(30);
    g_source_set_callback(source, callback, portal, NULL);
    g_source_attach(source, portal->context);
    g_source_unref(source);
}

static void save_file(GDBusConnection *bus, const gchar *sender, const gchar *path,
                      const gchar *interface, const gchar *method, GVariant *parameters,
                      GDBusMethodInvocation *invocation, gpointer data)
{
    (void)bus;
    MockPortal *portal = data;
    g_assert_cmpstr(path, ==, PORTAL_PATH);
    g_assert_cmpstr(interface, ==, INTERFACE);
    g_assert_cmpstr(method, ==, "SaveFile");
    const char *parent;
    const char *title;
    g_autoptr(GVariant) options = NULL;
    g_variant_get(parameters, "(&s&s@a{sv})", &parent, &title, &options);
    g_assert_cmpstr(parent, ==, "");
    g_assert_cmpstr(title, ==, "Portal request test");
    const char *token;
    g_assert_true(g_variant_lookup(options, "handle_token", "&s", &token));
    g_autofree char *sender_element = g_strdup(sender + 1);
    for (char *p = sender_element; *p != '\0'; p++) {
        if (*p == '.') {
            *p = '_';
        }
    }
    portal->request_path = g_strdup_printf("%s/request/%s/%s", PORTAL_PATH, sender_element, token);

    if (portal->scenario == WRONG_PATH) {
        g_dbus_method_invocation_return_value(invocation,
            g_variant_new("(o)", "/org/freedesktop/portal/desktop/request/wrong/handle"));
        return;
    }
    if (portal->scenario != LATE_RESPONSE
        && portal->scenario != BUS_DISCONNECT
        && portal->scenario != SERVICE_DISCONNECT) {
        emit_response(portal);
    }
    g_dbus_method_invocation_return_value(invocation, g_variant_new("(o)", portal->request_path));
    if (portal->scenario == LATE_RESPONSE) {
        schedule(portal, emit_later);
    } else if (portal->scenario == BUS_DISCONNECT || portal->scenario == SERVICE_DISCONNECT) {
        schedule(portal, disconnect_later);
    }
}

static gpointer serve(gpointer data)
{
    MockPortal *portal = data;
    portal->context = g_main_context_new();
    g_main_context_push_thread_default(portal->context);
    portal->loop = g_main_loop_new(portal->context, FALSE);
    g_autoptr(GError) error = NULL;
    portal->bus = g_dbus_connection_new_for_address_sync(
        g_test_dbus_get_bus_address(portal->test_bus),
        G_DBUS_CONNECTION_FLAGS_AUTHENTICATION_CLIENT | G_DBUS_CONNECTION_FLAGS_MESSAGE_BUS_CONNECTION,
        NULL, NULL, &error);
    g_assert_no_error(error);
    g_assert_nonnull(portal->bus);
    g_dbus_connection_set_exit_on_close(portal->bus, FALSE);
    g_autoptr(GVariant) owner = g_dbus_connection_call_sync(
        portal->bus, "org.freedesktop.DBus", "/org/freedesktop/DBus",
        "org.freedesktop.DBus", "RequestName", g_variant_new("(su)", PORTAL_NAME, 0),
        G_VARIANT_TYPE("(u)"), G_DBUS_CALL_FLAGS_NONE, 1000, NULL, &error);
    g_assert_no_error(error);
    g_assert_nonnull(owner);
    guint result;
    g_variant_get(owner, "(u)", &result);
    g_assert_cmpuint(result, ==, 1);
    g_autoptr(GDBusNodeInfo) info = g_dbus_node_info_new_for_xml(XML, &error);
    g_assert_no_error(error);
    const GDBusInterfaceVTable handlers = { .method_call = save_file };
    guint registration = g_dbus_connection_register_object(
        portal->bus, PORTAL_PATH, info->interfaces[0], &handlers, portal, NULL, &error);
    g_assert_no_error(error);
    g_assert_cmpuint(registration, >, 0);
    g_mutex_lock(&portal->lock);
    portal->ready = TRUE;
    g_cond_signal(&portal->condition);
    g_mutex_unlock(&portal->lock);

    g_main_loop_run(portal->loop);
    g_dbus_connection_unregister_object(portal->bus, registration);
    g_object_unref(portal->bus);
    g_main_loop_unref(portal->loop);
    g_main_context_pop_thread_default(portal->context);
    g_main_context_unref(portal->context);
    return NULL;
}

static gboolean stop_server(gpointer data)
{
    MockPortal *portal = data;
    g_main_loop_quit(portal->loop);
    return G_SOURCE_REMOVE;
}

static void run_scenario(gconstpointer data)
{
    if (!g_test_subprocess()) {
        g_test_trap_subprocess(NULL, 5 * G_USEC_PER_SEC, G_TEST_SUBPROCESS_DEFAULT);
        g_test_trap_assert_passed();
        return;
    }
    MockPortal portal = {
        .scenario = GPOINTER_TO_INT(data),
        .test_bus = g_test_dbus_new(G_TEST_DBUS_NONE),
        .ready = FALSE
    };
    g_mutex_init(&portal.lock);
    g_cond_init(&portal.condition);
    g_test_dbus_up(portal.test_bus);
    GThread *service = g_thread_new("mock-portal", serve, &portal);
    g_mutex_lock(&portal.lock);
    while (!portal.ready) {
        g_cond_wait(&portal.condition, &portal.lock);
    }
    g_mutex_unlock(&portal.lock);

    g_autofree char *token = portal_token();
    GVariantBuilder options;
    g_variant_builder_init(&options, G_VARIANT_TYPE_VARDICT);
    g_variant_builder_add(&options, "{sv}", "handle_token", g_variant_new_string(token));
    g_autoptr(GError) error = NULL;
    g_autofree char *request_path = NULL;
    g_autoptr(GVariant) response = portal_request(
        INTERFACE, "SaveFile", g_variant_new("(ss@a{sv})", "", "Portal request test",
        g_variant_builder_end(&options)), token, &request_path, &error);
    switch (portal.scenario) {
        case EARLY_RESPONSE:
        case LATE_RESPONSE: {
            g_assert_no_error(error);
            g_assert_nonnull(response);
            guint value;
            g_assert_true(g_variant_lookup(response, "test-value", "u", &value));
            g_assert_cmpuint(value, ==, 29);
            g_assert_cmpstr(request_path, ==, portal.request_path);
            break;
        }
        case CANCELLED_RESPONSE:
            g_assert_null(response);
            g_assert_error(error, G_IO_ERROR, G_IO_ERROR_CANCELLED);
            break;
        case FAILED_RESPONSE:
            g_assert_null(response);
            g_assert_error(error, G_IO_ERROR, G_IO_ERROR_FAILED);
            break;
        case WRONG_PATH:
        case MALFORMED_RESPONSE:
            g_assert_null(response);
            g_assert_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA);
            break;
        case BUS_DISCONNECT:
        case SERVICE_DISCONNECT:
            g_assert_null(response);
            g_assert_error(error, G_IO_ERROR, G_IO_ERROR_CLOSED);
            break;
    }
    g_main_context_invoke(portal.context, stop_server, &portal);
    g_thread_join(service);
    if (portal.scenario != BUS_DISCONNECT) {
        g_test_dbus_stop(portal.test_bus);
    }
    /* The helper's shared connection and GTestDBus live until this subprocess exits. */
    g_free(portal.request_path);
    g_cond_clear(&portal.condition);
    g_mutex_clear(&portal.lock);
}

int main(int argc, char **argv)
{
    g_test_init(&argc, &argv, NULL);
    g_test_add_data_func("/portal/response-before-reply", GINT_TO_POINTER(EARLY_RESPONSE), run_scenario);
    g_test_add_data_func("/portal/response-after-reply", GINT_TO_POINTER(LATE_RESPONSE), run_scenario);
    g_test_add_data_func("/portal/cancellation", GINT_TO_POINTER(CANCELLED_RESPONSE), run_scenario);
    g_test_add_data_func("/portal/failure", GINT_TO_POINTER(FAILED_RESPONSE), run_scenario);
    g_test_add_data_func("/portal/wrong-path", GINT_TO_POINTER(WRONG_PATH), run_scenario);
    g_test_add_data_func("/portal/malformed-response", GINT_TO_POINTER(MALFORMED_RESPONSE), run_scenario);
    g_test_add_data_func("/portal/bus-disconnect", GINT_TO_POINTER(BUS_DISCONNECT), run_scenario);
    g_test_add_data_func("/portal/service-disconnect", GINT_TO_POINTER(SERVICE_DISCONNECT), run_scenario);
    return g_test_run();
}
