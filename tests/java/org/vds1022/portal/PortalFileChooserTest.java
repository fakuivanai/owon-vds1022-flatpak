package org.vds1022.portal;

import java.io.File;
import java.io.IOException;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;
import javax.swing.JFileChooser;
import javax.swing.JPanel;
import javax.swing.SwingUtilities;
import javax.swing.filechooser.FileFilter;
import javax.swing.filechooser.FileNameExtensionFilter;

public final class PortalFileChooserTest {
    public static final class SuffixFilter extends FileFilter {
        private final String suffix;
        SuffixFilter(String suffix) { this.suffix = suffix; }
        public String getEnds() { return suffix; }
        @Override public boolean accept(File file) { return file.getName().endsWith("." + suffix); }
        @Override public String getDescription() { return suffix.toUpperCase(); }
    }

    private static void check(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }

    private static void formatsAndExactGrant() {
        AtomicInteger requests = new AtomicInteger();
        PortalFileChooser chooser = new PortalFileChooser((save, title, name, folder, filters,
            selected, multiple, directory, acceptLabel) -> {
            check(save && !multiple && !directory, "Export must use single-file SaveFile");
            check(filters.size() == 5, "Preserve all capture formats");
            if (requests.getAndIncrement() == 0) {
                check(name.endsWith(".png"), "Suggest the initial format extension");
                return new FilePortal.Selection(new File[] {new File("/tmp/document-grant/wave")}, 2);
            }
            check(name.equals("wave.csv") && selected == 2, "Request the final CSV filename");
            return new FilePortal.Selection(new File[] {new File("/tmp/document-grant/wave.csv")}, 2);
        });
        chooser.setAcceptAllFileFilterUsed(false);
        for (String suffix : new String[] {"png", "txt", "csv", "xls", "bin"})
            chooser.addChoosableFileFilter(new SuffixFilter(suffix));
        chooser.setFileFilter(chooser.getChoosableFileFilters()[0]);
        check(chooser.showSaveDialog(null) == JFileChooser.APPROVE_OPTION, "Approve exact authorized destination");
        check(requests.get() == 2, "Never append a suffix to a grant without another portal request");
        check(((SuffixFilter) chooser.getFileFilter()).getEnds().equals("csv"), "Restore the selected format object");
        check(chooser.getSelectedFile().getName().equals("wave.csv"), "Return the exact granted filename");
    }

    private static void openAndCancel() {
        File first = new File("/tmp/document-grant/captura ñ 💡.bin");
        File second = new File("/tmp/document-grant/second.bin");
        PortalFileChooser chooser = new PortalFileChooser((save, title, name, folder, filters,
            selected, multiple, directory, acceptLabel) -> {
            check(!save && multiple, "Use OpenFile with multiple selection");
            check(filters.get(0).patterns[0].equals("*.[bB][iI][nN]"), "Case-insensitive extension filter");
            return new FilePortal.Selection(new File[] {first, second}, 0);
        });
        chooser.setAcceptAllFileFilterUsed(false);
        chooser.setFileFilter(new FileNameExtensionFilter("Capture", "bin"));
        chooser.setMultiSelectionEnabled(true);
        check(chooser.showOpenDialog(null) == JFileChooser.APPROVE_OPTION, "Open approved files");
        check(chooser.getSelectedFiles().length == 2, "Keep multiple files");
        check(chooser.getSelectedFiles()[0].equals(first), "Preserve Unicode paths");
        PortalFileChooser canceled = new PortalFileChooser((a, b, c, d, e, f, g, h, i) -> null);
        check(canceled.showOpenDialog(null) == JFileChooser.CANCEL_OPTION, "Cancellation is not an error");
    }

    private static void recordingAndFailure() {
        PortalFileChooser chooser = new PortalFileChooser((save, title, name, folder, filters,
            selected, multiple, directory, acceptLabel) -> {
            check(save && filters.size() == 1 && name.endsWith(".cap"), "Recording must authorize a .cap save");
            check(acceptLabel.equals("Confirm"), "Preserve the approve label");
            return new FilePortal.Selection(new File[] {new File("/tmp/document-grant/recording.cap")}, 0);
        });
        chooser.addChoosableFileFilter(new SuffixFilter("cap"));
        check(FilePortal.recordingDialog(chooser, null, "Confirm") == JFileChooser.APPROVE_OPTION,
              "Custom recording dialog is a save operation");
        PortalFileChooser failed = new PortalFileChooser((a, b, c, d, e, f, g, h, i) -> {
            throw new IOException("No portal");
        });
        check(failed.showSaveDialog(null) == JFileChooser.ERROR_OPTION, "Portal failure must not fall back");
        failed.setAccessory(new JPanel());
        check(failed.showOpenDialog(null) == JFileChooser.ERROR_OPTION, "Do not silently discard accessories");
    }

    private static void eventDispatchThread() throws Exception {
        SwingUtilities.invokeAndWait(() -> {
            PortalFileChooser chooser = new PortalFileChooser((a, b, c, d, e, f, g, h, i) -> {
                check(!SwingUtilities.isEventDispatchThread(), "Portal request must not freeze Swing");
                return null;
            });
            check(chooser.showOpenDialog(null) == JFileChooser.CANCEL_OPTION, "Wait for worker on Swing thread");
        });
    }

    public static void main(String[] args) throws Exception {
        formatsAndExactGrant();
        openAndCancel();
        recordingAndFailure();
        eventDispatchThread();
        System.out.println("File chooser adapter tests passed.");
    }
}
