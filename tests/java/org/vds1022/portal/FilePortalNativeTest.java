package org.vds1022.portal;

import java.io.File;
import java.io.IOException;
import java.util.Arrays;
import java.util.List;
import javax.swing.filechooser.FileNameExtensionFilter;

/** Run only against the Rust file-portal-mock binary on a private D-Bus session. */
public final class FilePortalNativeTest {
    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }

    private static void rejected(List<FilePortal.Filter> filters, String reason) throws Exception {
        try {
            FilePortal.NATIVE.choose(false, "Captura ñ 💡", "", "", filters, 0,
                true, false, "");
            throw new AssertionError("The portal accepted an " + reason + " format");
        } catch (IOException expected) {
            check(expected.getMessage().contains(reason), "Reject the " + reason + " filter response");
        }
    }

    private interface NativeRequest {
        void run() throws IOException;
    }

    private static void invalidRequest(NativeRequest request, String reason) throws IOException {
        try {
            request.run();
            throw new AssertionError("The native chooser accepted invalid " + reason);
        } catch (IOException expected) {
            check(expected.getMessage().contains(reason), "Reject invalid " + reason + " before requesting access");
        }
    }

    public static void main(String[] args) throws Exception {
        List<FilePortal.Filter> filters = Arrays.asList(
            new FilePortal.Filter(new FileNameExtensionFilter("PNG ñ 💡", "png"),
                "PNG ñ 💡", new String[] {"*.[pP][nN][gG]"}, "png"),
            new FilePortal.Filter(new FileNameExtensionFilter("CSV ñ 💡", "csv"),
                "CSV ñ 💡", new String[] {"*.[cC][sS][vV]"}, "csv"));
        FilePortal.Selection save = FilePortal.NATIVE.choose(true, "Captura ñ 💡",
            "señal 💡.png", "/tmp/carpeta ñ 💡", filters, 0, false, false, "Guardar ñ 💡");
        check(save.filter == 1, "Restore the filter returned by the portal");
        check(save.files.length == 1, "Save exactly one file");
        check(save.files[0].equals(new File("/tmp/document-grant/señal ñ 💡.csv")),
              "Decode the portal URI without losing Unicode");
        FilePortal.Selection open = FilePortal.NATIVE.choose(false, "Captura ñ 💡",
            "", "", filters, 0, true, false, "");
        check(open.filter == 1 && open.files.length == 2, "Open all granted files");
        check(FilePortal.NATIVE.choose(false, "Captura ñ 💡", "", "", filters, 0,
            true, false, "") == null, "Map portal cancellation to chooser cancellation");
        FilePortal.Selection png = FilePortal.NATIVE.choose(true, "Captura ñ 💡",
            "señal 💡.png", "/tmp/carpeta ñ 💡", filters, 0, false, false, "Guardar ñ 💡");
        check(png.filter == 0, "Restore the PNG filter after KDE normalizes *.png");
        check(png.files[0].equals(new File("/tmp/document-grant/señal ñ 💡.png")),
            "Preserve the exact PNG file grant");
        FilePortal.Selection csv = FilePortal.NATIVE.choose(true, "Captura ñ 💡",
            "señal 💡.png", "/tmp/carpeta ñ 💡", filters, 0, false, false, "Guardar ñ 💡");
        check(csv.filter == 1, "Restore the CSV filter after KDE normalizes *.csv");
        check(csv.files[0].equals(new File("/tmp/document-grant/señal ñ 💡.csv")),
            "Preserve the exact CSV file grant");
        rejected(filters, "unknown");
        rejected(Arrays.asList(filters.get(1), filters.get(1)), "ambiguous");
        // These invalid arguments must fail before sending another portal request.
        invalidRequest(() -> FilePortal.NATIVE.choose(false, "bad\u0000title", "", "", filters,
            0, true, false, ""), "NUL");
        invalidRequest(() -> FilePortal.NATIVE.choose(false, "Captura ñ 💡", "bad\u0000name", "", filters,
            0, true, false, ""), "NUL");
        invalidRequest(() -> FilePortal.NATIVE.choose(false, "Captura ñ 💡", "", "bad\u0000folder", filters,
            0, true, false, ""), "NUL");
        invalidRequest(() -> FilePortal.NATIVE.choose(false, "Captura ñ 💡", "", "", filters,
            0, true, false, "bad\u0000label"), "NUL");
        invalidRequest(() -> FilePortal.NATIVE.choose(false, "bad\uD800title", "", "", filters,
            0, true, false, ""), "UTF-16");
        List<FilePortal.Filter> invalidLabel = Arrays.asList(new FilePortal.Filter(filters.get(0).original,
            "bad\u0000filter", new String[] {"*.png"}, "png"));
        invalidRequest(() -> FilePortal.NATIVE.choose(false, "Captura ñ 💡", "", "", invalidLabel,
            0, true, false, ""), "NUL");
        List<FilePortal.Filter> invalidPattern = Arrays.asList(new FilePortal.Filter(filters.get(0).original,
            "PNG", new String[] {"*.\u0000png"}, "png"));
        invalidRequest(() -> FilePortal.NATIVE.choose(false, "Captura ñ 💡", "", "", invalidPattern,
            0, true, false, ""), "NUL");
        invalidRequest(() -> FilePortal.NATIVE.choose(false, "Captura ñ 💡", "", "", filters,
            -1, true, false, ""), "filters");
        invalidRequest(() -> FilePortal.NATIVE.choose(false, "Captura ñ 💡", "", "", filters,
            filters.size(), true, false, ""), "filters");
        List<FilePortal.Filter> missingPatterns = Arrays.asList(new FilePortal.Filter(filters.get(0).original,
            "PNG", null, "png"));
        invalidRequest(() -> FilePortal.NATIVE.choose(false, "Captura ñ 💡", "", "", missingPatterns,
            0, true, false, ""), "patterns");
        System.out.println("Native file portal tests passed.");
    }
}
