/* Run the production USB JNI entry points against a fake portal and libusb.
 * Actual disposable file descriptors expose ownership leaks and double closes;
 * no desktop bus, USB device, or Java VM is used by this fixture. */
#define _POSIX_C_SOURCE 200809L
#include <gio/gio.h>
#include <gio/gunixfdlist.h>
#include <jni.h>
#include <libusb.h>
#include <fcntl.h>
#include <string.h>
#include <unistd.h>
#include "../src/native/portal.h"

static GDBusConnection *mock_connection(GError **error);
static gchar *mock_token(void);
static GVariant *mock_request(const char *, const char *, GVariant *, const char *, char **, GError **);
static GVariant *mock_call(GDBusConnection *, const gchar *, const gchar *, const gchar *,
                          const gchar *, GVariant *, const GVariantType *, GDBusCallFlags,
                          gint, GCancellable *, GError **);
static GVariant *mock_finish(GDBusConnection *, const gchar *, const gchar *, const gchar *,
                            const gchar *, GVariant *, const GVariantType *, GDBusCallFlags,
                            gint, GUnixFDList *, GUnixFDList **, GCancellable *, GError **);
static gint mock_fd_get(GUnixFDList *, gint, GError **);
static int mock_init(libusb_context **, const struct libusb_init_option *, int);
static void mock_exit(libusb_context *);
static int mock_wrap(libusb_context *, intptr_t, libusb_device_handle **);
static libusb_device *mock_device(libusb_device_handle *);
static int mock_descriptor(libusb_device *, struct libusb_device_descriptor *);
static int mock_configuration(libusb_device *, struct libusb_config_descriptor **);
static void mock_free_configuration(struct libusb_config_descriptor *);
static int mock_detach(libusb_device_handle *, int);
static int mock_claim(libusb_device_handle *, int);
static int mock_release_interface(libusb_device_handle *, int);
static void mock_usb_close(libusb_device_handle *);
static int mock_reset(libusb_device_handle *);
static int mock_bulk(libusb_device_handle *, unsigned char, unsigned char *, int, int *, unsigned int);
static int mock_close(int);

#define portal_connection mock_connection
#define portal_token mock_token
#define portal_request mock_request
#define g_dbus_connection_call_sync mock_call
#define g_dbus_connection_call_with_unix_fd_list_sync mock_finish
#define g_unix_fd_list_get mock_fd_get
#define libusb_init_context mock_init
#define libusb_exit mock_exit
#define libusb_wrap_sys_device mock_wrap
#define libusb_get_device mock_device
#define libusb_get_device_descriptor mock_descriptor
#define libusb_get_active_config_descriptor mock_configuration
#define libusb_free_config_descriptor mock_free_configuration
#define libusb_set_auto_detach_kernel_driver mock_detach
#define libusb_claim_interface mock_claim
#define libusb_release_interface mock_release_interface
#define libusb_close mock_usb_close
#define libusb_reset_device mock_reset
#define libusb_bulk_transfer mock_bulk
#define close mock_close
#include "../src/native/usb.c"
#undef g_unix_fd_list_get
#undef close

typedef struct {
    const char *id;
    const char *vendor;
    const char *product;
    gboolean present;
    gboolean readable;
    gboolean writable;
    gboolean cancelled;
    gboolean wrong_finish_id;
    gboolean invalid_fd_index;
    gboolean lose_release_reply;
    gboolean grant_active;
    guint16 descriptor_vendor;
    guint16 descriptor_product;
    int claim_result;
    int release_result;
    guint acquisitions;
    guint finishes;
    guint wraps;
    guint claims;
    guint interface_releases;
    guint usb_closes;
    guint fd_closes;
    guint grant_releases;
    guint transfers;
    guint resets;
} FakeDevice;

typedef struct { int fd; FakeDevice *owner; gboolean closed; } FakeFD;
typedef struct { FakeFD *descriptor; gboolean claimed; } FakeHandle;
typedef struct { jsize length; void *values; } FakeArray;

static FakeDevice fixture_devices[4];
static FakeDevice *requested_device;
static GPtrArray *fixture_fds;
static GPtrArray *jni_allocations;
static gchar *exception_class;
static gchar *exception_message;
static guint enumerations;
static guint initializations;
static guint context_exits;
static gboolean enumeration_error;

static void clear_exception(void)
{
    g_clear_pointer(&exception_class, g_free);
    g_clear_pointer(&exception_message, g_free);
}

static void assert_no_exception(void)
{
    if (exception_message != NULL) g_test_message("Unexpected JNI exception: %s", exception_message);
    g_assert_null(exception_class);
    g_assert_null(exception_message);
}

static void assert_usb_exception(void)
{
    g_assert_cmpstr(exception_class, ==, "ch/ntb/usb/USBException");
    g_assert_nonnull(exception_message);
    clear_exception();
}

static jclass JNICALL jni_find_class(JNIEnv *env, const char *name)
{
    (void) env;
    return (jclass) name;
}

static jint JNICALL jni_throw_new(JNIEnv *env, jclass type, const char *message)
{
    (void) env;
    g_assert_null(exception_class);
    exception_class = g_strdup((const char *) type);
    exception_message = g_strdup(message);
    return 0;
}

static void JNICALL jni_delete_ref(JNIEnv *env, jobject object)
{
    (void) env;
    (void) object;
}

static jboolean JNICALL jni_exception_check(JNIEnv *env)
{
    (void) env;
    return exception_class != NULL;
}

static jsize JNICALL jni_array_length(JNIEnv *env, jarray array)
{
    (void) env;
    return ((FakeArray *) array)->length;
}

static const char *JNICALL jni_string_chars(JNIEnv *env, jstring string, jboolean *copy)
{
    (void) env;
    (void) copy;
    return (const char *) string;
}

static void JNICALL jni_release_chars(JNIEnv *env, jstring string, const char *chars)
{
    (void) env;
    (void) string;
    (void) chars;
}

static void JNICALL jni_set_ints(JNIEnv *env, jintArray array, jsize start, jsize length, const jint *values)
{
    (void) env;
    FakeArray *target = (FakeArray *) array;
    g_assert_cmpint(start + length, <=, target->length);
    memcpy((jint *) target->values + start, values, (gsize) length * sizeof(jint));
}

static jobjectArray JNICALL jni_new_objects(JNIEnv *env, jsize length, jclass type, jobject initial)
{
    (void) env;
    (void) type;
    g_assert_null(initial);
    FakeArray *array = g_new(FakeArray, 1);
    *array = (FakeArray) { length, g_new0(gpointer, length) };
    g_ptr_array_add(jni_allocations, array->values);
    g_ptr_array_add(jni_allocations, array);
    return (jobjectArray) array;
}

static jstring JNICALL jni_new_string(JNIEnv *env, const char *value)
{
    (void) env;
    gchar *copy = g_strdup(value);
    g_ptr_array_add(jni_allocations, copy);
    return (jstring) copy;
}

static void JNICALL jni_set_object(JNIEnv *env, jobjectArray array, jsize index, jobject value)
{
    (void) env;
    FakeArray *target = (FakeArray *) array;
    g_assert_cmpint(index, <, target->length);
    ((gpointer *) target->values)[index] = value;
}

static void JNICALL jni_get_bytes(JNIEnv *env, jbyteArray array, jsize start, jsize length, jbyte *values)
{
    (void) env;
    FakeArray *source = (FakeArray *) array;
    g_assert_cmpint(start + length, <=, source->length);
    memcpy(values, (jbyte *) source->values + start, (gsize) length);
}

static void JNICALL jni_set_bytes(JNIEnv *env, jbyteArray array, jsize start, jsize length, const jbyte *values)
{
    (void) env;
    FakeArray *target = (FakeArray *) array;
    g_assert_cmpint(start + length, <=, target->length);
    memcpy((jbyte *) target->values + start, values, (gsize) length);
}

static const struct JNINativeInterface_ jni_functions = {
    .FindClass = jni_find_class,
    .ThrowNew = jni_throw_new,
    .DeleteLocalRef = jni_delete_ref,
    .ExceptionCheck = jni_exception_check,
    .GetArrayLength = jni_array_length,
    .GetStringUTFChars = jni_string_chars,
    .ReleaseStringUTFChars = jni_release_chars,
    .SetIntArrayRegion = jni_set_ints,
    .NewObjectArray = jni_new_objects,
    .NewStringUTF = jni_new_string,
    .SetObjectArrayElement = jni_set_object,
    .GetByteArrayRegion = jni_get_bytes,
    .SetByteArrayRegion = jni_set_bytes,
};
static JNIEnv jni = &jni_functions;

static FakeDevice *device_by_id(const char *id)
{
    for (guint i = 0; i < G_N_ELEMENTS(fixture_devices); i++) {
        if (g_str_equal(fixture_devices[i].id, id)) return &fixture_devices[i];
    }
    g_error("Unexpected portal ID: %s", id);
}

static GDBusConnection *mock_connection(GError **error)
{
    (void) error;
    return (GDBusConnection *) fixture_devices;
}

static gchar *mock_token(void)
{
    return g_strdup("test_request");
}

static GVariant *device_information(FakeDevice *device)
{
    GVariantBuilder properties, information;
    g_variant_builder_init(&properties, G_VARIANT_TYPE_VARDICT);
    g_variant_builder_add(&properties, "{sv}", "ID_VENDOR_ID", g_variant_new_string(device->vendor));
    g_variant_builder_add(&properties, "{sv}", "ID_MODEL_ID", g_variant_new_string(device->product));
    g_variant_builder_init(&information, G_VARIANT_TYPE_VARDICT);
    g_variant_builder_add(&information, "{sv}", "readable", g_variant_new_boolean(device->readable));
    g_variant_builder_add(&information, "{sv}", "writable", g_variant_new_boolean(device->writable));
    g_variant_builder_add(&information, "{sv}", "properties", g_variant_builder_end(&properties));
    return g_variant_builder_end(&information);
}

static GVariant *mock_call(GDBusConnection *connection, const gchar *bus_name,
                          const gchar *path, const gchar *interface, const gchar *method,
                          GVariant *parameters, const GVariantType *reply_type,
                          GDBusCallFlags flags, gint timeout, GCancellable *cancel, GError **error)
{
    (void) connection; (void) bus_name; (void) path; (void) reply_type;
    (void) flags; (void) timeout; (void) cancel;
    g_autoptr(GVariant) arguments = g_variant_ref_sink(parameters);
    g_assert_cmpstr(interface, ==, USB_INTERFACE);
    if (g_str_equal(method, "EnumerateDevices")) {
        enumerations++;
        if (enumeration_error) {
            g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_FAILED, "Mock enumeration failed");
            return NULL;
        }
        GVariantBuilder devices;
        g_variant_builder_init(&devices, G_VARIANT_TYPE("a(sa{sv})"));
        for (guint i = 0; i < G_N_ELEMENTS(fixture_devices); i++) {
            if (fixture_devices[i].present)
                g_variant_builder_add(&devices, "(s@a{sv})", fixture_devices[i].id,
                                      device_information(&fixture_devices[i]));
        }
        return g_variant_ref_sink(g_variant_new("(@a(sa{sv}))", g_variant_builder_end(&devices)));
    }
    g_assert_cmpstr(method, ==, "ReleaseDevices");
    g_autoptr(GVariant) ids = g_variant_get_child_value(arguments, 0);
    g_assert_cmpuint(g_variant_n_children(ids), ==, 1);
    const char *id;
    g_variant_get_child(ids, 0, "&s", &id);
    FakeDevice *device = device_by_id(id);
    g_assert_true(device->grant_active);
    device->grant_active = FALSE;
    device->grant_releases++;
    if (device->lose_release_reply) {
        /* The service processed release, but its reply never arrived. */
        g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_FAILED, "Mock release reply lost");
        return NULL;
    }
    return g_variant_ref_sink(g_variant_new("()"));
}

static GVariant *mock_request(const char *interface, const char *method,
                             GVariant *parameters, const char *token,
                             char **request_path, GError **error)
{
    g_autoptr(GVariant) arguments = g_variant_ref_sink(parameters);
    g_assert_cmpstr(interface, ==, USB_INTERFACE);
    g_assert_cmpstr(method, ==, "AcquireDevices");
    g_assert_nonnull(token);
    g_autoptr(GVariant) devices = g_variant_get_child_value(arguments, 1);
    g_assert_cmpuint(g_variant_n_children(devices), ==, 1);
    g_autoptr(GVariant) options = NULL;
    const char *id;
    g_variant_get_child(devices, 0, "(&s@a{sv})", &id, &options);
    gboolean writable = FALSE;
    g_assert_true(g_variant_lookup(options, "writable", "b", &writable));
    g_assert_true(writable);
    requested_device = device_by_id(id);
    requested_device->acquisitions++;
    if (requested_device->cancelled) {
        g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_CANCELLED, "Mock consent cancelled");
        return NULL;
    }
    g_assert_false(requested_device->grant_active);
    requested_device->grant_active = TRUE;
    *request_path = g_strdup("/org/freedesktop/portal/desktop/request/mock/test_request");
    return g_variant_ref_sink(g_variant_new_array(G_VARIANT_TYPE("{sv}"), NULL, 0));
}

static GVariant *mock_finish(GDBusConnection *connection, const gchar *bus_name,
                            const gchar *path, const gchar *interface, const gchar *method,
                            GVariant *parameters, const GVariantType *reply_type,
                            GDBusCallFlags flags, gint timeout, GUnixFDList *input_fds,
                            GUnixFDList **output_fds, GCancellable *cancel, GError **error)
{
    (void) connection; (void) bus_name; (void) path; (void) reply_type;
    (void) flags; (void) timeout; (void) input_fds; (void) cancel; (void) error;
    g_autoptr(GVariant) arguments = g_variant_ref_sink(parameters);
    g_assert_cmpstr(interface, ==, USB_INTERFACE);
    g_assert_cmpstr(method, ==, "FinishAcquireDevices");
    requested_device->finishes++;
    int fd = open("/dev/null", O_RDWR | O_CLOEXEC);
    g_assert_cmpint(fd, >=, 0);
    *output_fds = g_unix_fd_list_new();
    gint index = g_unix_fd_list_append(*output_fds, fd, NULL);
    g_assert_cmpint(index, >=, 0);
    g_assert_cmpint(close(fd), ==, 0);
    GVariantBuilder information, results;
    g_variant_builder_init(&information, G_VARIANT_TYPE_VARDICT);
    g_variant_builder_add(&information, "{sv}", "success", g_variant_new_boolean(TRUE));
    g_variant_builder_add(&information, "{sv}", "fd",
                          g_variant_new_handle(requested_device->invalid_fd_index ? index + 10 : index));
    g_variant_builder_init(&results, G_VARIANT_TYPE("a(sa{sv})"));
    g_variant_builder_add(&results, "(s@a{sv})",
                          requested_device->wrong_finish_id ? "scope-other" : requested_device->id,
                          g_variant_builder_end(&information));
    return g_variant_ref_sink(g_variant_new("(@a(sa{sv})b)", g_variant_builder_end(&results), TRUE));
}

static gint mock_fd_get(GUnixFDList *list, gint index, GError **error)
{
    int fd = g_unix_fd_list_get(list, index, error);
    if (fd >= 0) {
        FakeFD *descriptor = g_new(FakeFD, 1);
        *descriptor = (FakeFD) { fd, requested_device, FALSE };
        g_ptr_array_add(fixture_fds, descriptor);
    }
    return fd;
}

static FakeFD *live_fd(int fd)
{
    for (guint i = 0; i < fixture_fds->len; i++) {
        FakeFD *descriptor = fixture_fds->pdata[i];
        if (descriptor->fd == fd && !descriptor->closed) return descriptor;
    }
    g_error("Unknown or already closed descriptor: %d", fd);
}

static int mock_init(libusb_context **context, const struct libusb_init_option *options, int count)
{
    g_assert_cmpint(count, ==, 1);
    g_assert_cmpint(options[0].option, ==, LIBUSB_OPTION_NO_DEVICE_DISCOVERY);
    *context = (libusb_context *) fixture_devices;
    initializations++;
    return LIBUSB_SUCCESS;
}

static void mock_exit(libusb_context *context)
{
    g_assert_nonnull(context);
    context_exits++;
}

static int mock_wrap(libusb_context *context, intptr_t fd, libusb_device_handle **handle)
{
    g_assert_nonnull(context);
    FakeFD *descriptor = live_fd((int) fd);
    g_assert_cmpint(fcntl(descriptor->fd, F_GETFD), >=, 0);
    g_assert_true(descriptor->owner->grant_active);
    descriptor->owner->wraps++;
    FakeHandle *wrapped = g_new0(FakeHandle, 1);
    wrapped->descriptor = descriptor;
    *handle = (libusb_device_handle *) wrapped;
    return LIBUSB_SUCCESS;
}

static libusb_device *mock_device(libusb_device_handle *handle)
{
    return (libusb_device *) ((FakeHandle *) handle)->descriptor->owner;
}

static int mock_descriptor(libusb_device *usb, struct libusb_device_descriptor *descriptor)
{
    FakeDevice *device = (FakeDevice *) usb;
    *descriptor = (struct libusb_device_descriptor) {
        .idVendor = device->descriptor_vendor,
        .idProduct = device->descriptor_product,
        .bcdUSB = 0x0200,
    };
    return LIBUSB_SUCCESS;
}

static int mock_configuration(libusb_device *usb, struct libusb_config_descriptor **configuration)
{
    (void) usb;
    static const struct libusb_endpoint_descriptor endpoints[] = {
        { .bEndpointAddress = 0x81, .bmAttributes = LIBUSB_TRANSFER_TYPE_BULK, .wMaxPacketSize = 64 },
        { .bEndpointAddress = 0x03, .bmAttributes = LIBUSB_TRANSFER_TYPE_BULK, .wMaxPacketSize = 64 },
    };
    static const struct libusb_interface_descriptor alternate = {
        .bInterfaceNumber = 0, .bAlternateSetting = 0, .bNumEndpoints = 2, .endpoint = endpoints,
    };
    static const struct libusb_interface interface = { .altsetting = &alternate, .num_altsetting = 1 };
    *configuration = g_new0(struct libusb_config_descriptor, 1);
    **configuration = (struct libusb_config_descriptor) {
        .bConfigurationValue = 1, .bNumInterfaces = 1, .interface = &interface,
    };
    return LIBUSB_SUCCESS;
}

static void mock_free_configuration(struct libusb_config_descriptor *configuration)
{
    g_free(configuration);
}

static int mock_detach(libusb_device_handle *usb, int enable)
{
    (void) usb;
    g_assert_cmpint(enable, ==, 1);
    return LIBUSB_SUCCESS;
}

static int mock_claim(libusb_device_handle *usb, int interface)
{
    FakeHandle *handle = (FakeHandle *) usb;
    g_assert_cmpint(interface, ==, 0);
    g_assert_false(handle->claimed);
    FakeDevice *device = handle->descriptor->owner;
    device->claims++;
    if (device->claim_result == LIBUSB_SUCCESS) handle->claimed = TRUE;
    return device->claim_result;
}

static int mock_release_interface(libusb_device_handle *usb, int interface)
{
    FakeHandle *handle = (FakeHandle *) usb;
    g_assert_cmpint(interface, ==, 0);
    g_assert_true(handle->claimed);
    handle->claimed = FALSE;
    FakeDevice *device = handle->descriptor->owner;
    device->interface_releases++;
    return device->release_result;
}

static void mock_usb_close(libusb_device_handle *usb)
{
    FakeHandle *handle = (FakeHandle *) usb;
    g_assert_false(handle->claimed);
    handle->descriptor->owner->usb_closes++;
    g_free(handle);
}

static int mock_reset(libusb_device_handle *usb)
{
    ((FakeHandle *) usb)->descriptor->owner->resets++;
    return LIBUSB_SUCCESS;
}

static int mock_bulk(libusb_device_handle *usb, unsigned char endpoint, unsigned char *data,
                     int length, int *transferred, unsigned int timeout)
{
    FakeHandle *handle = (FakeHandle *) usb;
    g_assert_true(handle->claimed);
    g_assert_true(handle->descriptor->owner->grant_active);
    g_assert_true(endpoint == 0x81 || endpoint == 0x03);
    g_assert_cmpuint(timeout, >, 0);
    handle->descriptor->owner->transfers++;
    memset(data, 0x53, (gsize) length);
    *transferred = length;
    return LIBUSB_SUCCESS;
}

static int mock_close(int fd)
{
    FakeFD *descriptor = live_fd(fd);
    g_assert_cmpint(fcntl(fd, F_GETFD), >=, 0);
    descriptor->closed = TRUE;
    descriptor->owner->fd_closes++;
    int result = close(fd);
    g_assert_cmpint(result, ==, 0);
    g_assert_cmpint(fcntl(fd, F_GETFD), ==, -1);
    return result;
}

static void setup(void)
{
    g_assert_null(open_devices);
    g_assert_null(pending_grants);
    g_assert_null(usb_context);
    for (guint i = 0; i < G_N_ELEMENTS(fixture_devices); i++) {
        static const char *ids[] = { "scope-a", "scope-b", "unrelated", "unwritable" };
        fixture_devices[i] = (FakeDevice) {
            .id = ids[i], .vendor = i == 2 ? "5344" : "5345", .product = "1234",
            .present = TRUE, .readable = TRUE, .writable = i != 3,
            .descriptor_vendor = VENDOR_ID, .descriptor_product = PRODUCT_ID,
        };
    }
    fixture_fds = g_ptr_array_new_with_free_func(g_free);
    jni_allocations = g_ptr_array_new_with_free_func(g_free);
    requested_device = NULL;
    enumerations = initializations = context_exits = 0;
    enumeration_error = FALSE;
    clear_exception();
}

static void teardown(void)
{
    assert_no_exception();
    g_assert_null(open_devices);
    g_assert_null(pending_grants);
    JNI_USB(releaseNative)(&jni, NULL);
    assert_no_exception();
    for (guint i = 0; i < fixture_fds->len; i++)
        g_assert_true(((FakeFD *) fixture_fds->pdata[i])->closed);
    for (guint i = 0; i < G_N_ELEMENTS(fixture_devices); i++)
        g_assert_false(fixture_devices[i].grant_active);
    g_ptr_array_unref(fixture_fds);
    g_ptr_array_unref(jni_allocations);
}

static jlong open_scope(FakeDevice *device)
{
    jint metadata[8] = { 0 };
    FakeArray array = { 8, metadata };
    jlong token = JNI_USB(openNative)(&jni, NULL, (jstring) device->id, (jintArray) &array);
    if (token > 0) {
        static const jint expected[] = { 1, 0, 0, 0x81, 0x03, 1024, 1024, 0x0200 };
        g_assert_cmpmem(metadata, sizeof(metadata), expected, sizeof(expected));
        assert_no_exception();
    }
    return token;
}

static void probe_close(jlong token)
{
    JNI_USB(closeProbeNative)(&jni, NULL, token);
    assert_no_exception();
}

static void normal_close(jlong token)
{
    JNI_USB(closeNative)(&jni, NULL, token);
    assert_no_exception();
}

static void forget(FakeDevice *device)
{
    JNI_USB(forgetNative)(&jni, NULL, (jstring) device->id);
    assert_no_exception();
}

static void discover(void)
{
    FakeArray *ids = (FakeArray *) JNI_USB(enumerateNative)(&jni, NULL);
    assert_no_exception();
    g_assert_nonnull(ids);
    guint expected_count = 0;
    for (guint i = 0; i < 2; i++) {
        if (!fixture_devices[i].present) continue;
        gboolean found = FALSE;
        for (jsize index = 0; index < ids->length; index++)
            found |= g_str_equal(((char **) ids->values)[index], fixture_devices[i].id);
        g_assert_true(found);
        expected_count++;
    }
    g_assert_cmpint(ids->length, ==, (jsize) expected_count);
}

static void test_probe_reopen(void)
{
    setup();
    FakeDevice *scope = &fixture_devices[0];
    discover();
    g_assert_cmpuint(scope->acquisitions, ==, 0);
    jlong first = open_scope(scope);
    g_assert_cmpint(first, >, 0);
    jbyte buffer[4] = { 0 };
    FakeArray bytes = { 4, buffer };
    g_assert_cmpint(JNI_USB(transferNative)(&jni, NULL, first, (jbyteArray) &bytes, 4, 1000, TRUE), ==, 4);
    assert_no_exception();
    probe_close(first);
    g_assert_null(open_devices);
    g_assert_cmpuint(g_list_length(pending_grants), ==, 1);
    g_assert_cmpuint(scope->acquisitions, ==, 1);
    g_assert_cmpuint(scope->interface_releases, ==, 1);
    g_assert_cmpuint(scope->usb_closes, ==, 1);
    g_assert_cmpuint(scope->fd_closes, ==, 0);
    g_assert_cmpuint(scope->grant_releases, ==, 0);
    discover();
    jlong second = open_scope(scope);
    g_assert_cmpint(second, >, first);
    g_assert_null(pending_grants);
    g_assert_cmpuint(scope->acquisitions, ==, 1);
    g_assert_cmpuint(scope->finishes, ==, 1);
    g_assert_cmpuint(scope->wraps, ==, 2);
    g_assert_cmpuint(scope->claims, ==, 2);
    JNI_USB(closeNative)(&jni, NULL, first);
    assert_usb_exception();
    g_assert_cmpuint(scope->grant_releases, ==, 0);
    normal_close(second);
    g_assert_cmpuint(scope->fd_closes, ==, 1);
    g_assert_cmpuint(scope->grant_releases, ==, 1);
    g_assert_cmpuint(scope->transfers, ==, 1);
    forget(scope);
    g_assert_cmpuint(scope->grant_releases, ==, 1);
    teardown();
}

static void test_id_isolation(void)
{
    setup();
    FakeDevice *a = &fixture_devices[0], *b = &fixture_devices[1];
    discover();
    probe_close(open_scope(a));
    probe_close(open_scope(b));
    g_assert_cmpuint(g_list_length(pending_grants), ==, 2);
    jlong opened_b = open_scope(b);
    g_assert_cmpuint(g_list_length(pending_grants), ==, 1);
    normal_close(opened_b);
    g_assert_cmpuint(b->fd_closes, ==, 1);
    g_assert_cmpuint(b->grant_releases, ==, 1);
    g_assert_cmpuint(a->fd_closes, ==, 0);
    g_assert_true(a->grant_active);
    forget(a);
    g_assert_cmpuint(a->fd_closes, ==, 1);
    g_assert_cmpuint(a->grant_releases, ==, 1);
    g_assert_cmpuint(a->acquisitions, ==, 1);
    g_assert_cmpuint(b->acquisitions, ==, 1);
    g_assert_cmpuint(fixture_devices[2].acquisitions, ==, 0);
    g_assert_cmpuint(fixture_devices[3].acquisitions, ==, 0);
    teardown();
}

static void test_duplicate_open(void)
{
    setup();
    FakeDevice *scope = &fixture_devices[0];
    jlong token = open_scope(scope);
    g_assert_cmpint(open_scope(scope), ==, 0);
    assert_usb_exception();
    forget(scope);
    g_assert_cmpuint(scope->acquisitions, ==, 1);
    g_assert_cmpuint(scope->fd_closes, ==, 0);
    g_assert_cmpuint(scope->grant_releases, ==, 0);
    g_assert_true(scope->grant_active);
    normal_close(token);
    teardown();
}

static void test_ineligible_device(void)
{
    setup();
    for (guint i = 2; i < G_N_ELEMENTS(fixture_devices); i++) {
        g_assert_cmpint(open_scope(&fixture_devices[i]), ==, 0);
        assert_usb_exception();
        g_assert_cmpuint(fixture_devices[i].acquisitions, ==, 0);
        g_assert_cmpuint(fixture_devices[i].claims, ==, 0);
        g_assert_cmpuint(fixture_devices[i].grant_releases, ==, 0);
    }
    teardown();
}

static void test_rejected_probe(void)
{
    setup();
    FakeDevice *scope = &fixture_devices[0];
    /* Rejected/throwing model filters use normal close, never closeProbe. */
    normal_close(open_scope(scope));
    g_assert_null(pending_grants);
    g_assert_cmpuint(scope->fd_closes, ==, 1);
    g_assert_cmpuint(scope->grant_releases, ==, 1);
    normal_close(open_scope(scope));
    g_assert_cmpuint(scope->acquisitions, ==, 2);
    teardown();
}

static void test_cancelled(void)
{
    setup();
    FakeDevice *scope = &fixture_devices[0];
    scope->cancelled = TRUE;
    g_assert_cmpint(open_scope(scope), ==, 0);
    g_assert_cmpstr(exception_class, ==, "com/owon/uppersoft/vds/core/usb/CDevice$AccessCancelled");
    clear_exception();
    g_assert_cmpuint(scope->acquisitions, ==, 1);
    g_assert_cmpuint(scope->finishes, ==, 0);
    g_assert_cmpuint(scope->claims, ==, 0);
    g_assert_cmpuint(scope->grant_releases, ==, 0);
    teardown();
}

static void claim_failure(gboolean reused)
{
    setup();
    FakeDevice *scope = &fixture_devices[0];
    if (reused) probe_close(open_scope(scope));
    scope->claim_result = LIBUSB_ERROR_BUSY;
    g_assert_cmpint(open_scope(scope), ==, 0);
    assert_usb_exception();
    g_assert_cmpuint(scope->acquisitions, ==, 1);
    g_assert_cmpuint(scope->claims, ==, reused ? 2 : 1);
    g_assert_cmpuint(scope->usb_closes, ==, reused ? 2 : 1);
    g_assert_cmpuint(scope->fd_closes, ==, 1);
    g_assert_cmpuint(scope->grant_releases, ==, 1);
    teardown();
}

static void test_claim_failure(void) { claim_failure(FALSE); }
static void test_reused_claim_failure(void) { claim_failure(TRUE); }

static void test_probe_release_failure(void)
{
    setup();
    FakeDevice *scope = &fixture_devices[0];
    jlong token = open_scope(scope);
    scope->release_result = LIBUSB_ERROR_IO;
    JNI_USB(closeProbeNative)(&jni, NULL, token);
    assert_usb_exception();
    g_assert_cmpuint(scope->fd_closes, ==, 1);
    g_assert_cmpuint(scope->grant_releases, ==, 1);
    teardown();
}

static void test_unplugged_pending(void)
{
    setup();
    FakeDevice *scope = &fixture_devices[0];
    probe_close(open_scope(scope));
    scope->present = FALSE;
    discover();
    g_assert_cmpint(open_scope(scope), ==, 0);
    assert_usb_exception();
    g_assert_cmpuint(scope->acquisitions, ==, 1);
    g_assert_cmpuint(scope->fd_closes, ==, 1);
    g_assert_cmpuint(scope->grant_releases, ==, 1);
    forget(scope);
    teardown();
}

static void test_descriptor_change(void)
{
    setup();
    FakeDevice *scope = &fixture_devices[0];
    probe_close(open_scope(scope));
    scope->descriptor_product = 0x1235;
    g_assert_cmpint(open_scope(scope), ==, 0);
    assert_usb_exception();
    g_assert_cmpuint(scope->acquisitions, ==, 1);
    g_assert_cmpuint(scope->claims, ==, 1);
    g_assert_cmpuint(scope->fd_closes, ==, 1);
    g_assert_cmpuint(scope->grant_releases, ==, 1);
    teardown();
}

static void finish_failure(gboolean invalid_fd)
{
    setup();
    FakeDevice *scope = &fixture_devices[0];
    scope->invalid_fd_index = invalid_fd;
    scope->wrong_finish_id = !invalid_fd;
    g_assert_cmpint(open_scope(scope), ==, 0);
    assert_usb_exception();
    g_assert_cmpuint(scope->acquisitions, ==, 1);
    g_assert_cmpuint(scope->wraps, ==, 0);
    g_assert_cmpuint(scope->claims, ==, 0);
    g_assert_cmpuint(scope->grant_releases, ==, 1);
    teardown();
}

static void test_finish_id_mismatch(void) { finish_failure(FALSE); }
static void test_invalid_fd(void) { finish_failure(TRUE); }

static void test_shutdown(void)
{
    setup();
    FakeDevice *a = &fixture_devices[0], *b = &fixture_devices[1];
    probe_close(open_scope(a));
    g_assert_cmpint(open_scope(b), >, 0);
    JNI_USB(releaseNative)(&jni, NULL);
    assert_no_exception();
    g_assert_cmpuint(a->fd_closes, ==, 1);
    g_assert_cmpuint(b->fd_closes, ==, 1);
    g_assert_cmpuint(a->grant_releases, ==, 1);
    g_assert_cmpuint(b->grant_releases, ==, 1);
    g_assert_cmpuint(context_exits, ==, 1);
    teardown();
}

static void test_enumeration_failure(void)
{
    setup();
    FakeDevice *pending = &fixture_devices[0], *active = &fixture_devices[1];
    probe_close(open_scope(pending));
    jlong active_token = open_scope(active);
    pending->lose_release_reply = TRUE;
    enumeration_error = TRUE;
    g_assert_null(JNI_USB(enumerateNative)(&jni, NULL));
    g_assert_cmpstr(exception_message, ==, "Mock enumeration failed");
    assert_usb_exception();
    g_assert_null(pending_grants);
    g_assert_cmpuint(g_list_length(open_devices), ==, 1);
    g_assert_cmpuint(pending->fd_closes, ==, 1);
    g_assert_cmpuint(pending->grant_releases, ==, 1);
    g_assert_cmpuint(active->fd_closes, ==, 0);
    g_assert_cmpuint(active->grant_releases, ==, 0);
    jbyte buffer[4] = { 0 };
    FakeArray bytes = { 4, buffer };
    g_assert_cmpint(JNI_USB(transferNative)(&jni, NULL, active_token,
                                        (jbyteArray) &bytes, 4, 1000, TRUE), ==, 4);
    assert_no_exception();
    enumeration_error = FALSE;
    normal_close(active_token);
    teardown();
}

static void test_reset(void)
{
    setup();
    FakeDevice *scope = &fixture_devices[0];
    jlong token = open_scope(scope);
    JNI_USB(resetNative)(&jni, NULL, token);
    assert_no_exception();
    g_assert_cmpuint(scope->resets, ==, 1);
    g_assert_cmpuint(scope->fd_closes, ==, 1);
    g_assert_cmpuint(scope->grant_releases, ==, 1);
    normal_close(open_scope(scope));
    g_assert_cmpuint(scope->acquisitions, ==, 2);
    teardown();
}

int main(int argc, char **argv)
{
    g_test_init(&argc, &argv, NULL);
    g_test_add_func("/usb-grants/probe-reopen", test_probe_reopen);
    g_test_add_func("/usb-grants/id-isolation", test_id_isolation);
    g_test_add_func("/usb-grants/duplicate-open", test_duplicate_open);
    g_test_add_func("/usb-grants/ineligible-device", test_ineligible_device);
    g_test_add_func("/usb-grants/rejected-probe", test_rejected_probe);
    g_test_add_func("/usb-grants/cancelled", test_cancelled);
    g_test_add_func("/usb-grants/claim-failure", test_claim_failure);
    g_test_add_func("/usb-grants/reused-claim-failure", test_reused_claim_failure);
    g_test_add_func("/usb-grants/probe-release-failure", test_probe_release_failure);
    g_test_add_func("/usb-grants/unplugged-pending", test_unplugged_pending);
    g_test_add_func("/usb-grants/descriptor-change", test_descriptor_change);
    g_test_add_func("/usb-grants/finish-id-mismatch", test_finish_id_mismatch);
    g_test_add_func("/usb-grants/invalid-fd", test_invalid_fd);
    g_test_add_func("/usb-grants/shutdown", test_shutdown);
    g_test_add_func("/usb-grants/enumeration-failure", test_enumeration_failure);
    g_test_add_func("/usb-grants/reset", test_reset);
    return g_test_run();
}
