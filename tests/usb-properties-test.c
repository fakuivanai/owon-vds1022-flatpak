/* Exercise the portal device filter without a session bus or USB hardware. */
#include "../src/native/usb.c"

static GVariant *information(const char *vendor, const char *product,
                             gboolean readable, gboolean writable)
{
    GVariantBuilder properties;
    g_variant_builder_init(&properties, G_VARIANT_TYPE_VARDICT);
    if (vendor != NULL) {
        g_variant_builder_add(&properties, "{sv}", "ID_VENDOR_ID", g_variant_new_string(vendor));
    }
    if (product != NULL) {
        g_variant_builder_add(&properties, "{sv}", "ID_MODEL_ID", g_variant_new_string(product));
    }
    GVariantBuilder details;
    g_variant_builder_init(&details, G_VARIANT_TYPE_VARDICT);
    g_variant_builder_add(&details, "{sv}", "readable", g_variant_new_boolean(readable));
    g_variant_builder_add(&details, "{sv}", "writable", g_variant_new_boolean(writable));
    g_variant_builder_add(&details, "{sv}", "properties", g_variant_builder_end(&properties));
    return g_variant_ref_sink(g_variant_builder_end(&details));
}

static void test_filter(void)
{
    g_autoptr(GVariant) scope = information("5345", "1234", TRUE, TRUE);
    g_assert_true(scope_properties(scope));
    g_autoptr(GVariant) vendor = information("5344", "1234", TRUE, TRUE);
    g_assert_false(scope_properties(vendor));
    g_autoptr(GVariant) product = information("5345", "1235", TRUE, TRUE);
    g_assert_false(scope_properties(product));
    g_autoptr(GVariant) missing = information("5345", NULL, TRUE, TRUE);
    g_assert_false(scope_properties(missing));
    g_autoptr(GVariant) unreadable = information("5345", "1234", FALSE, TRUE);
    g_assert_false(scope_properties(unreadable));
    g_autoptr(GVariant) unwritable = information("5345", "1234", TRUE, FALSE);
    g_assert_false(scope_properties(unwritable));
    g_autoptr(GVariant) absent = g_variant_ref_sink(empty_options());
    g_assert_false(scope_properties(absent));
}

int main(int argc, char **argv)
{
    g_test_init(&argc, &argv, NULL);
    g_test_add_func("/usb/portal-properties", test_filter);
    return g_test_run();
}
