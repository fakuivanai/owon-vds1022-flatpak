#include "portal.h"

#include <string.h>

/* Request subscription order follows the portal's documented race prevention:
 * https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Request.html
 */
static const char *const PORTAL_NAME = "org.freedesktop.portal.Desktop";
static const char *const PORTAL_PATH = "/org/freedesktop/portal/desktop";
static GMutex connection_lock;
static GDBusConnection *connection;

GDBusConnection *portal_connection(GError **error)
{
    g_mutex_lock(&connection_lock);
    if (connection == NULL) {
        connection = g_bus_get_sync(G_BUS_TYPE_SESSION, NULL, error);
        if (connection != NULL)
            g_dbus_connection_set_exit_on_close(connection, FALSE);
    }
    if (connection != NULL && g_dbus_connection_is_closed(connection)) {
        g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_CLOSED,
                            "The desktop portal connection has closed");
        g_mutex_unlock(&connection_lock);
        return NULL;
    }
    GDBusConnection *result = connection;
    g_mutex_unlock(&connection_lock);
    return result;
}

gchar *portal_token(void)
{
    gchar *uuid = g_uuid_string_random();
    for (gchar *p = uuid; *p != '\0'; p++)
        if (*p == '-')
            *p = '_';
    gchar *token = g_strconcat("vds_", uuid, NULL);
    g_free(uuid);
    return token;
}

typedef struct {
    GMainLoop *loop;
    GDBusConnection *bus;
    GVariant *results;
    guint response;
    gboolean received;
    GError *failure;
} Request;

static void response_received(GDBusConnection *bus, const gchar *sender,
                              const gchar *path, const gchar *interface,
                              const gchar *signal, GVariant *parameters,
                              gpointer user_data)
{
    (void)bus;
    (void)sender;
    (void)path;
    (void)interface;
    (void)signal;
    Request *request = user_data;
    if (request->received)
        return;
    if (!g_variant_is_of_type(parameters, G_VARIANT_TYPE("(ua{sv})"))) {
        g_set_error_literal(&request->failure, G_IO_ERROR, G_IO_ERROR_INVALID_DATA,
                            "The portal returned a malformed response");
        g_main_loop_quit(request->loop);
        return;
    }
    g_variant_get(parameters, "(u@a{sv})", &request->response, &request->results);
    request->received = TRUE;
    g_main_loop_quit(request->loop);
}

static void portal_vanished(GDBusConnection *bus, const gchar *name,
                            gpointer user_data)
{
    (void)bus;
    (void)name;
    Request *request = user_data;
    if (!request->received && request->failure == NULL) {
        g_set_error_literal(&request->failure, G_IO_ERROR, G_IO_ERROR_CLOSED,
                            "The desktop portal stopped before responding");
        g_main_loop_quit(request->loop);
    }
}

static gboolean check_connection(gpointer user_data)
{
    Request *request = user_data;
    if (g_dbus_connection_is_closed(request->bus)) {
        g_main_loop_quit(request->loop);
        return G_SOURCE_REMOVE;
    }
    return G_SOURCE_CONTINUE;
}

GVariant *portal_request(const char *interface, const char *method,
                        GVariant *parameters, const char *token,
                        char **request_path, GError **error)
{
    g_return_val_if_fail(interface != NULL && method != NULL && token != NULL, NULL);
    if (request_path != NULL)
        *request_path = NULL;

    GDBusConnection *bus = portal_connection(error);
    if (bus == NULL)
        return NULL;

    const char *unique_name = g_dbus_connection_get_unique_name(bus);
    if (unique_name == NULL || unique_name[0] != ':') {
        g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_FAILED,
                            "The session bus has no unique connection name");
        return NULL;
    }
    gchar *sender = g_strdup(unique_name + 1);
    for (gchar *p = sender; *p != '\0'; p++)
        if (*p == '.')
            *p = '_';
    gchar *expected = g_strdup_printf("%s/request/%s/%s", PORTAL_PATH, sender, token);
    g_free(sender);

    GMainContext *context = g_main_context_new();
    g_main_context_push_thread_default(context);
    Request request = {
        .loop = g_main_loop_new(context, FALSE),
        .bus = bus,
        .results = NULL,
        .response = 2,
        .received = FALSE,
        .failure = NULL
    };
    guint subscription = g_dbus_connection_signal_subscribe(
        bus, PORTAL_NAME, "org.freedesktop.portal.Request", "Response",
        expected, NULL, G_DBUS_SIGNAL_FLAGS_NONE, response_received, &request, NULL);
    /* GDBusConnection's closed signal belongs to its construction context.
     * Check on this request's context so a disconnected bus cannot strand it.
     */
    GSource *connection_watch = g_timeout_source_new(250);
    g_source_set_callback(connection_watch, check_connection, &request, NULL);
    g_source_attach(connection_watch, context);

    GVariant *reply = g_dbus_connection_call_sync(
        bus, PORTAL_NAME, PORTAL_PATH, interface, method, parameters,
        G_VARIANT_TYPE("(o)"), G_DBUS_CALL_FLAGS_NONE, 30000, NULL, error);
    guint name_watch = 0;
    if (reply != NULL) {
        const char *returned;
        g_variant_get(reply, "(&o)", &returned);
        if (strcmp(returned, expected) != 0) {
            g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA,
                        "Portal returned an unexpected request path: %s", returned);
        } else {
            if (request_path != NULL)
                *request_path = g_strdup(returned);
            name_watch = g_bus_watch_name_on_connection(
                bus, PORTAL_NAME, G_BUS_NAME_WATCHER_FLAGS_NONE,
                NULL, portal_vanished, &request, NULL);
            if (!request.received && request.failure == NULL &&
                !g_dbus_connection_is_closed(bus))
                g_main_loop_run(request.loop);
        }
        g_variant_unref(reply);
    }

    if (name_watch != 0)
        g_bus_unwatch_name(name_watch);
    g_source_destroy(connection_watch);
    g_source_unref(connection_watch);
    g_dbus_connection_signal_unsubscribe(bus, subscription);
    g_main_loop_unref(request.loop);
    g_main_context_pop_thread_default(context);
    g_main_context_unref(context);
    g_free(expected);

    if (request.failure != NULL) {
        g_propagate_error(error, request.failure);
        g_clear_pointer(&request.results, g_variant_unref);
        return NULL;
    }
    if (error != NULL && *error != NULL) {
        g_clear_pointer(&request.results, g_variant_unref);
        return NULL;
    }
    if (!request.received) {
        g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_CLOSED,
                            "The portal connection closed before responding");
        return NULL;
    }
    if (request.response != 0) {
        g_clear_pointer(&request.results, g_variant_unref);
        g_set_error_literal(error, G_IO_ERROR,
                            request.response == 1 ? G_IO_ERROR_CANCELLED : G_IO_ERROR_FAILED,
                            request.response == 1 ? "The portal request was cancelled"
                                                  : "The portal request failed");
        return NULL;
    }
    return request.results;
}
