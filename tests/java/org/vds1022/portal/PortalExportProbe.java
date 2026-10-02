package org.vds1022.portal;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStreamReader;
import java.io.OutputStreamWriter;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;
import javax.swing.JFileChooser;
import javax.swing.SwingUtilities;
import javax.swing.filechooser.FileNameExtensionFilter;

/** Manual integration probe: writes only the file approved in the real save portal. */
public final class PortalExportProbe {
    private static final String MARKER = "sample,voltage\n0,0.25\n1,-0.25\n";

    public static void main(String[] args) throws Exception {
        if (args.length != 0) throw new IllegalArgumentException("This probe takes no arguments");
        AtomicInteger result = new AtomicInteger(JFileChooser.ERROR_OPTION);
        AtomicReference<File> destination = new AtomicReference<>();
        SwingUtilities.invokeAndWait(() -> {
            PortalFileChooser chooser = new PortalFileChooser();
            FileNameExtensionFilter csv = new FileNameExtensionFilter("CSV capture (*.csv)", "csv");
            chooser.setAcceptAllFileFilterUsed(false);
            chooser.setFileFilter(csv);
            chooser.setSelectedFile(new File("portal-export.csv"));
            chooser.setDialogTitle("Save the file portal test capture");
            int choice = chooser.showSaveDialog(null);
            result.set(choice);
            if (choice == JFileChooser.APPROVE_OPTION) {
                if (chooser.getFileFilter() != csv)
                    throw new AssertionError("The chooser did not preserve the selected CSV filter");
                File selected = chooser.getSelectedFile();
                if (!selected.getName().endsWith(".csv"))
                    throw new AssertionError("The portal did not authorize the final CSV filename");
                destination.set(selected);
            }
        });
        if (result.get() == JFileChooser.CANCEL_OPTION) {
            System.out.println("Export probe canceled; no file was written.");
            return;
        }
        if (result.get() != JFileChooser.APPROVE_OPTION)
            throw new IOException("The save portal failed; no file was written");
        File selected = destination.get();
        // The upstream CSV exporter likewise writes directly to the selected File:
        // https://github.com/florentbr/Owon-VDS1022/tree/a21cee14fc0807ce804657a26772784c4e47b5ef/lib
        try (OutputStreamWriter writer = new OutputStreamWriter(new FileOutputStream(selected),
                StandardCharsets.UTF_8)) {
            writer.write(MARKER);
        }
        StringBuilder readback = new StringBuilder();
        try (InputStreamReader reader = new InputStreamReader(new FileInputStream(selected),
                StandardCharsets.UTF_8)) {
            char[] buffer = new char[128];
            for (int count; (count = reader.read(buffer)) >= 0; )
                readback.append(buffer, 0, count);
        }
        if (!MARKER.equals(readback.toString()))
            throw new IOException("The CSV marker readback differs from the saved capture");
        System.out.println("File portal export passed: CSV filter preserved, exact .csv filename, write and readback.");
        System.out.println("Granted destination: " + selected.getAbsolutePath());
    }
}
