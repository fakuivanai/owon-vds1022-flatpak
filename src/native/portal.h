#ifndef VDS_PORTAL_H
#define VDS_PORTAL_H

#include <gio/gio.h>

/* The persistent connection keeps USB portal grants alive for this process. */
GDBusConnection *portal_connection(GError **error);
gchar *portal_token(void);
GVariant *portal_request(const char *interface, const char *method,
                        GVariant *parameters, const char *token,
                        char **request_path, GError **error);

#endif
