#include <gio/gio.h>
#include <stdio.h>
#include <string.h>

/* A private-session-bus fixture. It never opens files or contacts USB hardware. */
static const char XML[] =
    "<node><interface name='org.freedesktop.portal.FileChooser'>"
    "<method name='SaveFile'><arg type='s' direction='in'/><arg type='s' direction='in'/>"
    "<arg type='a{sv}' direction='in'/><arg type='o' direction='out'/></method>"
    "<method name='OpenFile'><arg type='s' direction='in'/><arg type='s' direction='in'/>"
    "<arg type='a{sv}' direction='in'/><arg type='o' direction='out'/></method>"
    "</interface></node>";
static unsigned requests;

static void choose(GDBusConnection *bus, const gchar *sender, const gchar *path,
    const gchar *interface, const gchar *method, GVariant *parameters,
    GDBusMethodInvocation *invocation, gpointer data)
{
    (void) path; (void) interface; (void) data;
    const gchar *parent, *title, *token;
    GVariant *options;
    g_variant_get(parameters, "(&s&s@a{sv})", &parent, &title, &options);
    g_assert_cmpstr(parent, ==, "");
    g_assert_cmpstr(title, ==, "Captura ñ 💡");
    g_assert_true(g_variant_lookup(options, "handle_token", "&s", &token));
    gboolean modal;
    g_assert_true(g_variant_lookup(options, "modal", "b", &modal));
    g_assert_true(modal);
    GVariant *filters = g_variant_lookup_value(options, "filters", G_VARIANT_TYPE("a(sa(us))"));
    g_assert_nonnull(filters);
    g_assert_cmpuint(g_variant_n_children(filters), ==, 2);
    GVariant *initial = g_variant_lookup_value(options, "current_filter", G_VARIANT_TYPE("(sa(us))"));
    GVariant *first = g_variant_get_child_value(filters, 0);
    g_assert_true(g_variant_equal(initial, first));
    g_variant_unref(first);
    g_variant_unref(initial);
    if (strcmp(method, "SaveFile") == 0) {
        const gchar *name, *label;
        g_assert_true(g_variant_lookup(options, "current_name", "&s", &name));
        g_assert_cmpstr(name, ==, "señal 💡.png");
        g_assert_true(g_variant_lookup(options, "accept_label", "&s", &label));
        g_assert_cmpstr(label, ==, "Guardar ñ 💡");
        GVariant *folder = g_variant_lookup_value(options, "current_folder", G_VARIANT_TYPE("ay"));
        g_assert_nonnull(folder);
        g_assert_cmpstr(g_variant_get_bytestring(folder), ==, "/tmp/carpeta ñ 💡");
        g_variant_unref(folder);
        g_assert_null(g_variant_lookup_value(options, "multiple", NULL));
    } else {
        gboolean multiple, directory;
        g_assert_true(g_variant_lookup(options, "multiple", "b", &multiple));
        g_assert_true(multiple);
        g_assert_true(g_variant_lookup(options, "directory", "b", &directory));
        g_assert_false(directory);
    }
    gchar *peer = g_strdup(sender + 1);
    for (gchar *p = peer; *p; p++) if (*p == '.') *p = '_';
    gchar *request = g_strdup_printf("/org/freedesktop/portal/desktop/request/%s/%s", peer, token);
    g_free(peer);
    g_dbus_method_invocation_return_value(invocation, g_variant_new("(o)", request));
    GVariantBuilder results;
    g_variant_builder_init(&results, G_VARIANT_TYPE_VARDICT);
    if (requests != 2) {
        GVariantBuilder uris;
        g_variant_builder_init(&uris, G_VARIANT_TYPE("as"));
        gchar *uri = g_filename_to_uri(requests == 3
            ? "/tmp/document-grant/señal ñ 💡.png" : "/tmp/document-grant/señal ñ 💡.csv", NULL, NULL);
        g_variant_builder_add(&uris, "s", uri);
        g_free(uri);
        if (strcmp(method, "OpenFile") == 0)
            g_variant_builder_add(&uris, "s", "file:///tmp/document-grant/second.csv");
        g_variant_builder_add(&results, "{sv}", "uris", g_variant_builder_end(&uris));
        GVariant *selected = g_variant_get_child_value(filters, requests == 3 ? 0 : 1);
        if (requests >= 3) {
            /* Reproduce KDE's case-pair glob normalization and an unknown format. */
            const gchar *label;
            g_variant_get_child(selected, 0, "&s", &label);
            GVariantBuilder patterns;
            g_variant_builder_init(&patterns, G_VARIANT_TYPE("a(us)"));
            g_variant_builder_add(&patterns, "(us)", 0U,
                requests == 3 ? "*.png" : requests == 5 ? "*.xls" : "*.csv");
            GVariant *normalized = g_variant_ref_sink(g_variant_new("(s@a(us))", label,
                g_variant_builder_end(&patterns)));
            g_variant_unref(selected);
            selected = normalized;
        }
        g_variant_builder_add(&results, "{sv}", "current_filter", selected);
        g_variant_unref(selected);
    }
    GError *error = NULL;
    g_assert_true(g_dbus_connection_emit_signal(bus, sender, request,
        "org.freedesktop.portal.Request", "Response",
        g_variant_new("(u@a{sv})", requests == 2 ? 1U : 0U,
                      g_variant_builder_end(&results)), &error));
    g_assert_no_error(error);
    requests++;
    g_free(request);
    g_variant_unref(filters);
    g_variant_unref(options);
}

int main(void)
{
    GError *error = NULL;
    GDBusConnection *bus = g_bus_get_sync(G_BUS_TYPE_SESSION, NULL, &error);
    g_assert_no_error(error);
    GVariant *reply = g_dbus_connection_call_sync(bus, "org.freedesktop.DBus",
        "/org/freedesktop/DBus", "org.freedesktop.DBus", "RequestName",
        g_variant_new("(su)", "org.freedesktop.portal.Desktop", 0U),
        G_VARIANT_TYPE("(u)"), G_DBUS_CALL_FLAGS_NONE, 30000, NULL, &error);
    g_assert_no_error(error);
    guint result;
    g_variant_get(reply, "(u)", &result);
    g_assert_cmpuint(result, ==, 1U);
    g_variant_unref(reply);
    GDBusNodeInfo *info = g_dbus_node_info_new_for_xml(XML, &error);
    g_assert_no_error(error);
    const GDBusInterfaceVTable vtable = { .method_call = choose };
    g_assert_cmpuint(g_dbus_connection_register_object(bus,
        "/org/freedesktop/portal/desktop", info->interfaces[0], &vtable,
        NULL, NULL, &error), >, 0);
    g_assert_no_error(error);
    puts("ready");
    fflush(stdout);
    GMainLoop *loop = g_main_loop_new(NULL, FALSE);
    g_main_loop_run(loop);
    return 0;
}
