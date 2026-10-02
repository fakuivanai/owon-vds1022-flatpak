#include <jni.h>
#include <gio/gio.h>
#include <string.h>
#include "portal.h"

/* FileChooser protocol, including document-portal URI grants:
 * https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.FileChooser.html */

static gchar *from_java(JNIEnv *env, jstring value, GError **error)
{
    if (!value) return g_strdup("");
    jsize length = (*env)->GetStringLength(env, value);
    const jchar *chars = (*env)->GetStringChars(env, value, NULL);
    if (!chars) return NULL;
    for (jsize i = 0; i < length; i++) {
        if (chars[i] == 0) {
            (*env)->ReleaseStringChars(env, value, chars);
            g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT,
                                "File portal strings must not contain NUL");
            return NULL;
        }
    }
    gchar *text = g_utf16_to_utf8((const gunichar2 *) chars, length, NULL, NULL, error);
    (*env)->ReleaseStringChars(env, value, chars);
    return text;
}

static jstring to_java(JNIEnv *env, const gchar *text)
{
    glong length;
    gunichar2 *chars = g_utf8_to_utf16(text, -1, NULL, &length, NULL);
    if (!chars) {
        jclass exception = (*env)->FindClass(env, "java/io/IOException");
        if (exception) (*env)->ThrowNew(env, exception, "File portal returned invalid UTF-8");
        return NULL;
    }
    jstring result = (*env)->NewString(env, (const jchar *) chars, (jsize) length);
    g_free(chars);
    return result;
}

static void throw_error(JNIEnv *env, const gchar *message)
{
    if ((*env)->ExceptionCheck(env)) return;
    jclass type = (*env)->FindClass(env, "java/io/IOException");
    if (!type) return;
    jmethodID constructor = (*env)->GetMethodID(env, type, "<init>", "(Ljava/lang/String;)V");
    jstring text = to_java(env, message);
    if (constructor && text) {
        jobject exception = (*env)->NewObject(env, type, constructor, text);
        if (exception) (*env)->Throw(env, exception);
    }
}

static gchar *normalize_suffix_glob(const gchar *pattern)
{
    /* KDE normalizes complete ASCII case-pair suffixes, *.[pP][nN][gG] -> *.png:
     * https://github.com/KDE/xdg-desktop-portal-kde/blob/v6.6.6/src/filechooser.cpp
     * Accept only that round trip; retain every other pattern exactly. */
    gsize length = strlen(pattern);
    if (!g_str_has_prefix(pattern, "*.") || length <= 2 || (length - 2) % 4 != 0)
        return g_strdup(pattern);
    gchar *normalized = g_strdup(pattern);
    gsize output = 2;
    for (gsize i = 2; i < length; i += 4) {
        gchar first = pattern[i + 1], second = pattern[i + 2];
        if (pattern[i] != '[' || pattern[i + 3] != ']'
            || !((g_ascii_islower(first) && g_ascii_isupper(second))
                 || (g_ascii_isupper(first) && g_ascii_islower(second)))
            || g_ascii_tolower(first) != g_ascii_tolower(second)) {
            g_free(normalized);
            return g_strdup(pattern);
        }
        normalized[output++] = g_ascii_tolower(first);
    }
    normalized[output] = '\0';
    return normalized;
}

static gboolean same_filter(GVariant *expected, GVariant *returned)
{
    const gchar *expected_label, *returned_label;
    GVariant *expected_patterns, *returned_patterns;
    g_variant_get(expected, "(&s@a(us))", &expected_label, &expected_patterns);
    g_variant_get(returned, "(&s@a(us))", &returned_label, &returned_patterns);
    gboolean equal = strcmp(expected_label, returned_label) == 0
        && g_variant_n_children(expected_patterns) == g_variant_n_children(returned_patterns);
    for (gsize i = 0; equal && i < g_variant_n_children(expected_patterns); i++) {
        guint expected_type, returned_type;
        const gchar *expected_pattern, *returned_pattern;
        g_variant_get_child(expected_patterns, i, "(u&s)", &expected_type, &expected_pattern);
        g_variant_get_child(returned_patterns, i, "(u&s)", &returned_type, &returned_pattern);
        gchar *left = expected_type == 0 ? normalize_suffix_glob(expected_pattern) : g_strdup(expected_pattern);
        gchar *right = returned_type == 0 ? normalize_suffix_glob(returned_pattern) : g_strdup(returned_pattern);
        equal = expected_type == returned_type && strcmp(left, right) == 0;
        g_free(left);
        g_free(right);
    }
    g_variant_unref(returned_patterns);
    g_variant_unref(expected_patterns);
    return equal;
}

JNIEXPORT jobjectArray JNICALL Java_org_vds1022_portal_FilePortal_chooseNative(
    JNIEnv *env, jclass type, jboolean save, jstring jtitle, jstring jname,
    jstring jfolder, jobjectArray jlabels, jobjectArray jpatterns,
    jint selected_filter, jboolean multiple, jboolean directory, jstring jaccept_label)
{
    (void) type;
    GError *error = NULL;
    gchar *title = from_java(env, jtitle, &error);
    gchar *name = NULL;
    gchar *folder = NULL;
    gchar *accept_label = NULL;
    gchar *token = NULL;
    GVariant *filters = NULL;
    GVariant *results = NULL;
    GVariant *uris = NULL;
    GVariant *current_filter = NULL;
    jobjectArray answer = NULL;
    if (!title) goto done;
    name = from_java(env, jname, &error);
    if (!name) goto done;
    folder = from_java(env, jfolder, &error);
    if (!folder) goto done;
    accept_label = from_java(env, jaccept_label, &error);
    if (!accept_label) goto done;
    if (!jlabels || !jpatterns) {
        g_set_error_literal(&error, G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT, "Missing file filters");
        goto done;
    }
    jsize count = (*env)->GetArrayLength(env, jlabels);
    if (count == 0 || (*env)->GetArrayLength(env, jpatterns) != count
        || selected_filter < 0 || selected_filter >= count) {
        g_set_error_literal(&error, G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT, "Invalid file filters");
        goto done;
    }
    GVariantBuilder filter_builder;
    g_variant_builder_init(&filter_builder, G_VARIANT_TYPE("a(sa(us))"));
    for (jsize i = 0; i < count; i++) {
        jstring jlabel = (*env)->GetObjectArrayElement(env, jlabels, i);
        gchar *label = from_java(env, jlabel, &error);
        if (jlabel) (*env)->DeleteLocalRef(env, jlabel);
        jobjectArray patterns = (*env)->GetObjectArrayElement(env, jpatterns, i);
        if (!label || !patterns) {
            g_free(label);
            if (patterns) (*env)->DeleteLocalRef(env, patterns);
            g_variant_builder_clear(&filter_builder);
            if (!error && !(*env)->ExceptionCheck(env))
                g_set_error_literal(&error, G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT, "Missing filter patterns");
            goto done;
        }
        GVariantBuilder pattern_builder;
        g_variant_builder_init(&pattern_builder, G_VARIANT_TYPE("a(us)"));
        jsize patterns_count = (*env)->GetArrayLength(env, patterns);
        for (jsize j = 0; j < patterns_count; j++) {
            jstring jpattern = (*env)->GetObjectArrayElement(env, patterns, j);
            gchar *pattern = from_java(env, jpattern, &error);
            if (jpattern) (*env)->DeleteLocalRef(env, jpattern);
            if (!pattern) {
                g_free(label);
                (*env)->DeleteLocalRef(env, patterns);
                g_variant_builder_clear(&pattern_builder);
                g_variant_builder_clear(&filter_builder);
                goto done;
            }
            g_variant_builder_add(&pattern_builder, "(us)", 0U, pattern);
            g_free(pattern);
        }
        (*env)->DeleteLocalRef(env, patterns);
        g_variant_builder_add(&filter_builder, "(s@a(us))", label,
                              g_variant_builder_end(&pattern_builder));
        g_free(label);
    }
    filters = g_variant_ref_sink(g_variant_builder_end(&filter_builder));
    GVariantBuilder options;
    g_variant_builder_init(&options, G_VARIANT_TYPE_VARDICT);
    token = portal_token();
    g_variant_builder_add(&options, "{sv}", "handle_token", g_variant_new_string(token));
    g_variant_builder_add(&options, "{sv}", "modal", g_variant_new_boolean(TRUE));
    g_variant_builder_add(&options, "{sv}", "filters", filters);
    GVariant *initial_filter = g_variant_get_child_value(filters, selected_filter);
    g_variant_builder_add(&options, "{sv}", "current_filter", initial_filter);
    g_variant_unref(initial_filter);
    if (*accept_label) g_variant_builder_add(&options, "{sv}", "accept_label", g_variant_new_string(accept_label));
    if (*folder) g_variant_builder_add(&options, "{sv}", "current_folder", g_variant_new_bytestring(folder));
    if (save) {
        if (*name) g_variant_builder_add(&options, "{sv}", "current_name", g_variant_new_string(name));
    } else {
        g_variant_builder_add(&options, "{sv}", "multiple", g_variant_new_boolean(multiple));
        g_variant_builder_add(&options, "{sv}", "directory", g_variant_new_boolean(directory));
    }
    results = portal_request("org.freedesktop.portal.FileChooser", save ? "SaveFile" : "OpenFile",
        g_variant_new("(ss@a{sv})", "", title, g_variant_builder_end(&options)), token, NULL, &error);
    if (!results) goto done;
    uris = g_variant_lookup_value(results, "uris", G_VARIANT_TYPE("as"));
    if (!uris || g_variant_n_children(uris) == 0 || (save && g_variant_n_children(uris) != 1)) {
        g_set_error_literal(&error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA, "File portal returned invalid file selection");
        goto done;
    }
    current_filter = g_variant_lookup_value(results, "current_filter", G_VARIANT_TYPE("(sa(us))"));
    if (current_filter) {
        guint matches = 0;
        for (jsize i = 0; i < count; i++) {
            GVariant *candidate = g_variant_get_child_value(filters, i);
            gboolean equal = same_filter(candidate, current_filter);
            g_variant_unref(candidate);
            if (equal) { selected_filter = i; matches++; }
        }
        if (matches != 1) {
            g_set_error_literal(&error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA,
                matches == 0 ? "File portal returned an unknown format" : "File portal returned an ambiguous format");
            goto done;
        }
    }
    jclass string_type = (*env)->FindClass(env, "java/lang/String");
    if (!string_type) goto done;
    answer = (*env)->NewObjectArray(env, (jsize) g_variant_n_children(uris) + 1, string_type, NULL);
    if (!answer) goto done;
    gchar *index = g_strdup_printf("%d", selected_filter);
    jstring jindex = to_java(env, index);
    g_free(index);
    if (!jindex) goto done;
    (*env)->SetObjectArrayElement(env, answer, 0, jindex);
    (*env)->DeleteLocalRef(env, jindex);
    for (gsize i = 0; i < g_variant_n_children(uris); i++) {
        const gchar *uri;
        g_variant_get_child(uris, i, "&s", &uri);
        jstring juri = to_java(env, uri);
        if (!juri) goto done;
        (*env)->SetObjectArrayElement(env, answer, (jsize) i + 1, juri);
        (*env)->DeleteLocalRef(env, juri);
        if ((*env)->ExceptionCheck(env)) goto done;
    }
done:
    if (error) {
        if (!g_error_matches(error, G_IO_ERROR, G_IO_ERROR_CANCELLED)) throw_error(env, error->message);
        answer = NULL;
        g_error_free(error);
    }
    g_clear_pointer(&current_filter, g_variant_unref);
    g_clear_pointer(&uris, g_variant_unref);
    g_clear_pointer(&results, g_variant_unref);
    g_clear_pointer(&filters, g_variant_unref);
    g_free(token);
    g_free(accept_label);
    g_free(folder);
    g_free(name);
    g_free(title);
    return answer;
}
