/* USB portal protocol:
 * https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Usb.html
 * FD ownership / wrapping:
 * https://libusb.sourceforge.io/api-1.0/group__libusb__dev.html
 * Original implementation; no vendor native code is reused.
 */
#include "portal.h"
#include <gio/gunixfdlist.h>
#include <jni.h>
#include <libusb.h>
#include <stdint.h>
#include <unistd.h>

#define USB_INTERFACE "org.freedesktop.portal.Usb"
#define PORTAL_NAME "org.freedesktop.portal.Desktop"
#define PORTAL_PATH "/org/freedesktop/portal/desktop"
#define VENDOR_ID 0x5345
#define PRODUCT_ID 0x1234
#define JNI_USB(name) Java_com_owon_uppersoft_vds_core_usb_CDevice_##name

typedef struct {
    guint64 token;
    gchar *portal_id;
    int fd;
    libusb_device_handle *usb;
    guint8 read_endpoint;
    guint8 write_endpoint;
} OpenDevice;

/* An accepted discovery probe keeps its grant, but no libusb handle or claim. */
typedef struct {
    gchar *portal_id;
    int fd;
} PendingGrant;

/* GLib permits zero-initialized static GMutex instances. */
static GMutex usb_lock;
static libusb_context *usb_context;
static GList *open_devices;
static GList *pending_grants;
static guint64 next_token = 1;

static void throw_java(JNIEnv *env, const char *class_name, const char *message)
{
    jclass exception = (*env)->FindClass(env, class_name);
    if (exception != NULL) {
        (*env)->ThrowNew(env, exception, message);
        (*env)->DeleteLocalRef(env, exception);
    }
}

static void throw_usb(JNIEnv *env, const char *message)
{
    throw_java(env, "ch/ntb/usb/USBException", message);
}

static void throw_portal(JNIEnv *env, GError *error)
{
    const char *class_name = g_error_matches(error, G_IO_ERROR, G_IO_ERROR_CANCELLED)
        ? "com/owon/uppersoft/vds/core/usb/CDevice$AccessCancelled"
        : "ch/ntb/usb/USBException";
    throw_java(env, class_name, error->message);
}

static gboolean init_usb(GError **error)
{
    if (usb_context != NULL) {
        return TRUE;
    }
    const struct libusb_init_option option = {
        .option = LIBUSB_OPTION_NO_DEVICE_DISCOVERY,
    };
    int result = libusb_init_context(&usb_context, &option, 1);
    if (result != LIBUSB_SUCCESS) {
        g_set_error(error, G_IO_ERROR, G_IO_ERROR_FAILED,
                    "Cannot initialize USB library: %s", libusb_error_name(result));
        return FALSE;
    }
    return TRUE;
}

static GVariant *empty_options(void)
{
    return g_variant_new_array(G_VARIANT_TYPE("{sv}"), NULL, 0);
}

static GVariant *enumerate_devices(GError **error)
{
    GDBusConnection *connection = portal_connection(error);
    if (connection == NULL) {
        return NULL;
    }
    g_autoptr(GVariant) reply = g_dbus_connection_call_sync(
        connection, PORTAL_NAME, PORTAL_PATH, USB_INTERFACE, "EnumerateDevices",
        g_variant_new("(@a{sv})", empty_options()), G_VARIANT_TYPE("(a(sa{sv}))"),
        G_DBUS_CALL_FLAGS_NONE, 10000, NULL, error);
    if (reply == NULL) {
        return NULL;
    }
    return g_variant_get_child_value(reply, 0);
}

static gboolean scope_properties(GVariant *information)
{
    gboolean readable = FALSE;
    gboolean writable = FALSE;
    g_variant_lookup(information, "readable", "b", &readable);
    g_variant_lookup(information, "writable", "b", &writable);
    g_autoptr(GVariant) properties = g_variant_lookup_value(
        information, "properties", G_VARIANT_TYPE_VARDICT);
    const gchar *vendor;
    const gchar *product;
    return readable && writable && properties != NULL
        && g_variant_lookup(properties, "ID_VENDOR_ID", "&s", &vendor)
        && g_variant_lookup(properties, "ID_MODEL_ID", "&s", &product)
        && g_ascii_strcasecmp(vendor, "5345") == 0
        && g_ascii_strcasecmp(product, "1234") == 0;
}

static gboolean scope_available(const char *id, GError **error)
{
    g_autoptr(GVariant) devices = enumerate_devices(error);
    if (devices == NULL) {
        return FALSE;
    }
    for (gsize index = 0; index < g_variant_n_children(devices); index++) {
        g_autoptr(GVariant) device = g_variant_get_child_value(devices, index);
        g_autoptr(GVariant) information = NULL;
        const char *candidate;
        g_variant_get(device, "(&s@a{sv})", &candidate, &information);
        if (g_str_equal(candidate, id) && scope_properties(information)) {
            return TRUE;
        }
    }
    g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_NOT_FOUND,
                        "The selected scope is unavailable through the USB portal");
    return FALSE;
}

static gboolean release_grant(const char *id, GError **error)
{
    GDBusConnection *connection = portal_connection(error);
    if (connection == NULL) {
        return FALSE;
    }
    const gchar *ids[] = { id, NULL };
    g_autoptr(GVariant) reply = g_dbus_connection_call_sync(
        connection, PORTAL_NAME, PORTAL_PATH, USB_INTERFACE, "ReleaseDevices",
        g_variant_new("(^as@a{sv})", ids, empty_options()), NULL,
        G_DBUS_CALL_FLAGS_NONE, 10000, NULL, error);
    return reply != NULL;
}

/* The request helper keeps its D-Bus connection alive for the lifetime of grants. */
static int acquire_fd(const char *id, GError **error)
{
    GVariantBuilder access;
    g_variant_builder_init(&access, G_VARIANT_TYPE_VARDICT);
    g_variant_builder_add(&access, "{sv}", "writable", g_variant_new_boolean(TRUE));
    GVariantBuilder devices;
    g_variant_builder_init(&devices, G_VARIANT_TYPE("a(sa{sv})"));
    g_variant_builder_add(&devices, "(s@a{sv})", id, g_variant_builder_end(&access));
    g_autofree gchar *token = portal_token();
    GVariantBuilder options;
    g_variant_builder_init(&options, G_VARIANT_TYPE_VARDICT);
    g_variant_builder_add(&options, "{sv}", "handle_token", g_variant_new_string(token));
    g_autofree gchar *request_path = NULL;
    g_autoptr(GVariant) response = portal_request(
        USB_INTERFACE, "AcquireDevices",
        g_variant_new("(s@a(sa{sv})@a{sv})", "", g_variant_builder_end(&devices),
                      g_variant_builder_end(&options)),
        token, &request_path, error);
    if (response == NULL) {
        return -1;
    }

    GDBusConnection *connection = portal_connection(error);
    if (connection == NULL) {
        return -1;
    }
    int acquired_fd = -1;
    gboolean finished = FALSE;
    while (!finished) {
        g_autoptr(GUnixFDList) fd_list = NULL;
        g_autoptr(GVariant) reply = g_dbus_connection_call_with_unix_fd_list_sync(
            connection, PORTAL_NAME, PORTAL_PATH, USB_INTERFACE, "FinishAcquireDevices",
            g_variant_new("(o@a{sv})", request_path, empty_options()),
            G_VARIANT_TYPE("(a(sa{sv})b)"), G_DBUS_CALL_FLAGS_NONE, 10000,
            NULL, &fd_list, NULL, error);
        if (reply == NULL) {
            if (acquired_fd >= 0) {
                close(acquired_fd);
            }
            release_grant(id, NULL);
            return -1;
        }
        g_autoptr(GVariant) results = NULL;
        g_variant_get(reply, "(@a(sa{sv})b)", &results, &finished);
        for (gsize index = 0; index < g_variant_n_children(results); index++) {
            g_autoptr(GVariant) result = g_variant_get_child_value(results, index);
            g_autoptr(GVariant) information = NULL;
            const char *candidate;
            g_variant_get(result, "(&s@a{sv})", &candidate, &information);
            if (!g_str_equal(candidate, id)) {
                continue;
            }
            gboolean success = FALSE;
            gint32 fd_index;
            if (!g_variant_lookup(information, "success", "b", &success) || !success) {
                const char *message = "The USB portal did not grant access to the scope";
                g_variant_lookup(information, "error", "&s", &message);
                g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_PERMISSION_DENIED, message);
            } else if (fd_list == NULL
                       || !g_variant_lookup(information, "fd", "h", &fd_index)
                       || fd_index < 0 || fd_index >= g_unix_fd_list_get_length(fd_list)
                       || acquired_fd >= 0) {
                g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA,
                                    "The USB portal returned an invalid file descriptor result");
            } else {
                acquired_fd = g_unix_fd_list_get(fd_list, fd_index, error);
            }
            if (error != NULL && *error != NULL) {
                break;
            }
        }
        if (error != NULL && *error != NULL) {
            if (acquired_fd >= 0) {
                close(acquired_fd);
            }
            release_grant(id, NULL);
            return -1;
        }
    }
    if (acquired_fd < 0) {
        g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA,
                            "The USB portal did not return the scope file descriptor");
        release_grant(id, NULL);
    }
    return acquired_fd;
}

static OpenDevice *find_device(JNIEnv *env, jlong token)
{
    for (GList *entry = open_devices; entry != NULL; entry = entry->next) {
        OpenDevice *device = entry->data;
        if (token > 0 && device->token == (guint64) token) {
            return device;
        }
    }
    throw_usb(env, "The USB device handle is closed or invalid");
    return NULL;
}

static gboolean close_device(OpenDevice *device, GError **error)
{
    open_devices = g_list_remove(open_devices, device);
    int result = libusb_release_interface(device->usb, 0);
    libusb_close(device->usb);
    close(device->fd);
    gboolean released = release_grant(device->portal_id, error);
    if (released && result != LIBUSB_SUCCESS && result != LIBUSB_ERROR_NO_DEVICE) {
        g_set_error(error, G_IO_ERROR, G_IO_ERROR_FAILED,
                    "Cannot release scope interface: %s", libusb_error_name(result));
        released = FALSE;
    }
    g_free(device->portal_id);
    g_free(device);
    return released;
}

static int take_pending_fd(const char *id)
{
    for (GList *entry = pending_grants; entry != NULL; entry = entry->next) {
        PendingGrant *grant = entry->data;
        if (g_str_equal(grant->portal_id, id)) {
            int fd = grant->fd;
            pending_grants = g_list_delete_link(pending_grants, entry);
            g_free(grant->portal_id);
            g_free(grant);
            return fd;
        }
    }
    return -1;
}

static gboolean close_pending_grant(PendingGrant *grant, GError **error)
{
    pending_grants = g_list_remove(pending_grants, grant);
    close(grant->fd);
    gboolean released = release_grant(grant->portal_id, error);
    g_free(grant->portal_id);
    g_free(grant);
    return released;
}

static gboolean describe_device(libusb_device_handle *handle, jint metadata[8], GError **error)
{
    libusb_device *device = libusb_get_device(handle);
    struct libusb_device_descriptor descriptor;
    int result = libusb_get_device_descriptor(device, &descriptor);
    if (result != LIBUSB_SUCCESS) {
        g_set_error(error, G_IO_ERROR, G_IO_ERROR_FAILED,
                    "Cannot read scope descriptor: %s", libusb_error_name(result));
        return FALSE;
    }
    if (descriptor.idVendor != VENDOR_ID || descriptor.idProduct != PRODUCT_ID) {
        g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_PERMISSION_DENIED,
                            "The granted USB descriptor is not a supported scope");
        return FALSE;
    }
    struct libusb_config_descriptor *configuration = NULL;
    result = libusb_get_active_config_descriptor(device, &configuration);
    if (result != LIBUSB_SUCCESS) {
        g_set_error(error, G_IO_ERROR, G_IO_ERROR_FAILED,
                    "Cannot read active scope configuration: %s", libusb_error_name(result));
        return FALSE;
    }
    gboolean found = FALSE;
    for (guint index = 0; index < configuration->bNumInterfaces && !found; index++) {
        const struct libusb_interface *interface = &configuration->interface[index];
        for (int alternate = 0; alternate < interface->num_altsetting && !found; alternate++) {
            const struct libusb_interface_descriptor *setting = &interface->altsetting[alternate];
            if (setting->bInterfaceNumber != 0 || setting->bAlternateSetting != 0) {
                continue;
            }
            int read_endpoint = -1;
            int write_endpoint = -1;
            int read_size = 0;
            int write_size = 0;
            gboolean ambiguous = FALSE;
            for (guint ep = 0; ep < setting->bNumEndpoints; ep++) {
                const struct libusb_endpoint_descriptor *endpoint = &setting->endpoint[ep];
                if ((endpoint->bmAttributes & LIBUSB_TRANSFER_TYPE_MASK) != LIBUSB_TRANSFER_TYPE_BULK) {
                    continue;
                }
                if ((endpoint->bEndpointAddress & LIBUSB_ENDPOINT_DIR_MASK) == LIBUSB_ENDPOINT_IN) {
                    ambiguous |= read_endpoint >= 0;
                    read_endpoint = endpoint->bEndpointAddress;
                    read_size = endpoint->wMaxPacketSize * 16;
                } else {
                    ambiguous |= write_endpoint >= 0;
                    write_endpoint = endpoint->bEndpointAddress;
                    write_size = endpoint->wMaxPacketSize * 16;
                }
            }
            if (!ambiguous && read_endpoint >= 0 && write_endpoint >= 0
                && read_size > 0 && write_size > 0) {
                metadata[0] = configuration->bConfigurationValue;
                metadata[1] = setting->bInterfaceNumber;
                metadata[2] = setting->bAlternateSetting;
                metadata[3] = read_endpoint;
                metadata[4] = write_endpoint;
                metadata[5] = read_size;
                metadata[6] = write_size;
                metadata[7] = descriptor.bcdUSB;
                found = TRUE;
            }
        }
    }
    libusb_free_config_descriptor(configuration);
    if (!found) {
        g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA,
                            "Scope interface 0 does not have one bulk input and one bulk output");
    }
    return found;
}

JNIEXPORT void JNICALL JNI_USB(initNative)(JNIEnv *env, jclass type)
{
    (void) type;
    g_mutex_lock(&usb_lock);
    g_autoptr(GError) error = NULL;
    if (!init_usb(&error)) {
        throw_usb(env, error->message);
    }
    g_mutex_unlock(&usb_lock);
}

JNIEXPORT void JNICALL JNI_USB(releaseNative)(JNIEnv *env, jclass type)
{
    (void) type;
    g_mutex_lock(&usb_lock);
    g_autoptr(GError) first_error = NULL;
    while (open_devices != NULL) {
        g_autoptr(GError) error = NULL;
        if (!close_device(open_devices->data, &error) && first_error == NULL) {
            first_error = g_steal_pointer(&error);
        }
    }
    while (pending_grants != NULL) {
        g_autoptr(GError) error = NULL;
        if (!close_pending_grant(pending_grants->data, &error) && first_error == NULL) {
            first_error = g_steal_pointer(&error);
        }
    }
    if (usb_context != NULL) {
        libusb_exit(usb_context);
        usb_context = NULL;
    }
    if (first_error != NULL) {
        throw_portal(env, first_error);
    }
    g_mutex_unlock(&usb_lock);
}

JNIEXPORT jobjectArray JNICALL JNI_USB(enumerateNative)(JNIEnv *env, jclass type)
{
    (void) type;
    g_mutex_lock(&usb_lock);
    g_autoptr(GError) error = NULL;
    g_autoptr(GVariant) devices = enumerate_devices(&error);
    if (devices == NULL) {
        while (pending_grants != NULL) {
            close_pending_grant(pending_grants->data, NULL);
        }
        throw_portal(env, error);
        g_mutex_unlock(&usb_lock);
        return NULL;
    }
    g_autoptr(GPtrArray) ids = g_ptr_array_new_with_free_func(g_free);
    for (gsize index = 0; index < g_variant_n_children(devices); index++) {
        g_autoptr(GVariant) device = g_variant_get_child_value(devices, index);
        g_autoptr(GVariant) information = NULL;
        const char *id;
        g_variant_get(device, "(&s@a{sv})", &id, &information);
        if (scope_properties(information)) {
            g_ptr_array_add(ids, g_strdup(id));
        }
    }
    jclass string_class = (*env)->FindClass(env, "java/lang/String");
    jobjectArray result = string_class != NULL
        ? (*env)->NewObjectArray(env, (jsize) ids->len, string_class, NULL) : NULL;
    if (string_class != NULL) {
        (*env)->DeleteLocalRef(env, string_class);
    }
    if (result != NULL) {
        for (guint index = 0; index < ids->len; index++) {
            jstring id = (*env)->NewStringUTF(env, ids->pdata[index]);
            if (id == NULL) {
                break;
            }
            (*env)->SetObjectArrayElement(env, result, (jsize) index, id);
            (*env)->DeleteLocalRef(env, id);
            if ((*env)->ExceptionCheck(env)) {
                break;
            }
        }
    }
    g_mutex_unlock(&usb_lock);
    return result;
}

JNIEXPORT jlong JNICALL JNI_USB(openNative)(JNIEnv *env, jclass type, jstring id, jintArray metadata_array)
{
    (void) type;
    if (id == NULL || metadata_array == NULL || (*env)->GetArrayLength(env, metadata_array) != 8) {
        throw_java(env, "java/lang/IllegalArgumentException", "Invalid scope ID or metadata buffer");
        return 0;
    }
    const char *portal_id = (*env)->GetStringUTFChars(env, id, NULL);
    if (portal_id == NULL) {
        return 0;
    }
    g_mutex_lock(&usb_lock);
    g_autoptr(GError) error = NULL;
    int fd = -1;
    libusb_device_handle *usb = NULL;
    jint metadata[8];
    jlong token = 0;
    for (GList *entry = open_devices; entry != NULL; entry = entry->next) {
        OpenDevice *device = entry->data;
        if (g_str_equal(device->portal_id, portal_id)) {
            g_set_error_literal(&error, G_IO_ERROR, G_IO_ERROR_BUSY, "This scope is already open");
            goto done;
        }
    }
    /* Consume before validation so every failure also drops a stale probe grant. */
    fd = take_pending_fd(portal_id);
    if (!init_usb(&error) || !scope_available(portal_id, &error)) {
        goto done;
    }
    if (fd < 0) {
        fd = acquire_fd(portal_id, &error);
    }
    if (fd < 0) {
        goto done;
    }
    int result = libusb_wrap_sys_device(usb_context, (intptr_t) fd, &usb);
    if (result != LIBUSB_SUCCESS) {
        g_set_error(&error, G_IO_ERROR, G_IO_ERROR_FAILED,
                    "Cannot wrap granted USB descriptor: %s", libusb_error_name(result));
        goto done;
    }
    if (!describe_device(usb, metadata, &error)) {
        goto done;
    }
    result = libusb_set_auto_detach_kernel_driver(usb, 1);
    if (result != LIBUSB_SUCCESS && result != LIBUSB_ERROR_NOT_SUPPORTED) {
        g_set_error(&error, G_IO_ERROR, G_IO_ERROR_FAILED,
                    "Cannot enable scope interface detachment: %s", libusb_error_name(result));
        goto done;
    }
    result = libusb_claim_interface(usb, 0);
    if (result != LIBUSB_SUCCESS) {
        g_set_error(&error, G_IO_ERROR, G_IO_ERROR_FAILED,
                    "Cannot claim scope interface 0: %s", libusb_error_name(result));
        goto done;
    }
    (*env)->SetIntArrayRegion(env, metadata_array, 0, 8, metadata);
    if ((*env)->ExceptionCheck(env)) {
        libusb_release_interface(usb, 0);
        goto done;
    }
    if (next_token > G_MAXINT64) {
        libusb_release_interface(usb, 0);
        g_set_error_literal(&error, G_IO_ERROR, G_IO_ERROR_FAILED, "USB handle sequence exhausted");
        goto done;
    }
    OpenDevice *device = g_new(OpenDevice, 1);
    *device = (OpenDevice) {
        .token = next_token++,
        .portal_id = g_strdup(portal_id),
        .fd = fd,
        .usb = usb,
        .read_endpoint = (guint8) metadata[3],
        .write_endpoint = (guint8) metadata[4],
    };
    open_devices = g_list_prepend(open_devices, device);
    token = (jlong) device->token;
    usb = NULL;
    fd = -1;
done:
    if (usb != NULL) {
        libusb_close(usb);
    }
    if (fd >= 0) {
        close(fd);
        release_grant(portal_id, NULL);
    }
    if (error != NULL) {
        throw_portal(env, error);
    }
    g_mutex_unlock(&usb_lock);
    (*env)->ReleaseStringUTFChars(env, id, portal_id);
    return token;
}

JNIEXPORT void JNICALL JNI_USB(closeNative)(JNIEnv *env, jclass type, jlong token)
{
    (void) type;
    g_mutex_lock(&usb_lock);
    OpenDevice *device = find_device(env, token);
    g_autoptr(GError) error = NULL;
    if (device != NULL && !close_device(device, &error)) {
        throw_portal(env, error);
    }
    g_mutex_unlock(&usb_lock);
}

JNIEXPORT void JNICALL JNI_USB(closeProbeNative)(JNIEnv *env, jclass type, jlong token)
{
    (void) type;
    g_mutex_lock(&usb_lock);
    OpenDevice *device = find_device(env, token);
    if (device != NULL) {
        open_devices = g_list_remove(open_devices, device);
        int result = libusb_release_interface(device->usb, 0);
        libusb_close(device->usb);
        if (result == LIBUSB_SUCCESS) {
            /* openNative consumes this exact ID before creating another handle. */
            PendingGrant *grant = g_new(PendingGrant, 1);
            *grant = (PendingGrant) {
                .portal_id = g_steal_pointer(&device->portal_id),
                .fd = device->fd,
            };
            pending_grants = g_list_prepend(pending_grants, grant);
        } else {
            close(device->fd);
            g_autoptr(GError) error = NULL;
            if (release_grant(device->portal_id, &error)) {
                g_set_error(&error, G_IO_ERROR, G_IO_ERROR_FAILED,
                            "Cannot release scope probe interface: %s", libusb_error_name(result));
            }
            throw_portal(env, error);
        }
        g_free(device->portal_id);
        g_free(device);
    }
    g_mutex_unlock(&usb_lock);
}

JNIEXPORT void JNICALL JNI_USB(forgetNative)(JNIEnv *env, jclass type, jstring id)
{
    (void) type;
    if (id == NULL) {
        throw_java(env, "java/lang/IllegalArgumentException", "Invalid scope ID");
        return;
    }
    const char *portal_id = (*env)->GetStringUTFChars(env, id, NULL);
    if (portal_id == NULL) {
        return;
    }
    g_mutex_lock(&usb_lock);
    for (GList *entry = pending_grants; entry != NULL; entry = entry->next) {
        PendingGrant *grant = entry->data;
        if (g_str_equal(grant->portal_id, portal_id)) {
            g_autoptr(GError) error = NULL;
            if (!close_pending_grant(grant, &error)) {
                throw_portal(env, error);
            }
            break;
        }
    }
    g_mutex_unlock(&usb_lock);
    (*env)->ReleaseStringUTFChars(env, id, portal_id);
}

JNIEXPORT void JNICALL JNI_USB(resetNative)(JNIEnv *env, jclass type, jlong token)
{
    (void) type;
    g_mutex_lock(&usb_lock);
    OpenDevice *device = find_device(env, token);
    if (device != NULL) {
        int result = libusb_reset_device(device->usb);
        g_autoptr(GError) error = NULL;
        gboolean closed = close_device(device, &error);
        if (result != LIBUSB_SUCCESS && result != LIBUSB_ERROR_NO_DEVICE) {
            throw_usb(env, libusb_error_name(result));
        } else if (!closed) {
            throw_portal(env, error);
        }
    }
    g_mutex_unlock(&usb_lock);
}

JNIEXPORT jint JNICALL JNI_USB(transferNative)(
    JNIEnv *env, jclass type, jlong token, jbyteArray data, jint size, jint timeout, jboolean read)
{
    (void) type;
    if (data == NULL || size <= 0 || size > (*env)->GetArrayLength(env, data) || timeout <= 0) {
        throw_java(env, "java/lang/IllegalArgumentException", "Invalid USB transfer buffer, size or timeout");
        return -1;
    }
    g_mutex_lock(&usb_lock);
    OpenDevice *device = find_device(env, token);
    if (device == NULL) {
        g_mutex_unlock(&usb_lock);
        return -1;
    }
    g_autofree unsigned char *buffer = g_malloc((gsize) size);
    if (!read) {
        (*env)->GetByteArrayRegion(env, data, 0, size, (jbyte *) buffer);
        if ((*env)->ExceptionCheck(env)) {
            g_mutex_unlock(&usb_lock);
            return -1;
        }
    }
    int transferred = 0;
    int result = libusb_bulk_transfer(
        device->usb, read ? device->read_endpoint : device->write_endpoint,
        buffer, size, &transferred, (unsigned int) timeout);
    /* libusb 0.1 returns partial progress even if a bulk operation times out. */
    if (result != LIBUSB_SUCCESS && !(result == LIBUSB_ERROR_TIMEOUT && transferred > 0)) {
        throw_usb(env, libusb_error_name(result));
        transferred = -1;
    } else if (read) {
        (*env)->SetByteArrayRegion(env, data, 0, transferred, (jbyte *) buffer);
    }
    g_mutex_unlock(&usb_lock);
    return transferred;
}
