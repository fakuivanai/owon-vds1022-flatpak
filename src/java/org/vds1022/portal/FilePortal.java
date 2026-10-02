package org.vds1022.portal;

import java.awt.Component;
import java.awt.EventQueue;
import java.awt.SecondaryLoop;
import java.awt.Toolkit;
import java.io.File;
import java.io.IOException;
import java.lang.reflect.Method;
import java.net.URI;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.FutureTask;
import javax.swing.JFileChooser;
import javax.swing.filechooser.FileFilter;
import javax.swing.filechooser.FileNameExtensionFilter;

/** File selection with host permission supplied by the FileChooser portal. */
public final class FilePortal {
    private FilePortal() {}

    static final class Filter {
        final FileFilter original;
        final String label;
        final String[] patterns;
        final String extension;

        Filter(FileFilter original, String label, String[] patterns, String extension) {
            this.original = original;
            this.label = label;
            this.patterns = patterns;
            this.extension = extension;
        }
    }

    static final class Selection {
        final File[] files;
        final int filter;

        Selection(File[] files, int filter) {
            if (files.length == 0) throw new IllegalArgumentException("No selected files");
            this.files = files;
            this.filter = filter;
        }
    }

    interface Backend {
        Selection choose(boolean save, String title, String name, String folder,
                         List<Filter> filters, int selectedFilter, boolean multiple,
                         boolean directory, String acceptLabel) throws IOException;
    }

    private static final class NativeLibrary {
        static { System.loadLibrary("vdsportal"); }
        static void load() {}
    }

    static final Backend NATIVE = (save, title, name, folder, filters, selected, multiple,
                                  directory, acceptLabel) -> {
        NativeLibrary.load();
        String[] labels = new String[filters.size()];
        String[][] patterns = new String[filters.size()][];
        for (int i = 0; i < filters.size(); i++) {
            labels[i] = filters.get(i).label;
            patterns[i] = filters.get(i).patterns;
        }
        String[] result = chooseNative(save, title, name, folder, labels, patterns,
                                       selected, multiple, directory, acceptLabel);
        if (result == null) return null;
        if (result.length < 2) throw new IOException("File portal returned no files");
        int selectedFilter;
        try {
            selectedFilter = Integer.parseInt(result[0]);
        } catch (NumberFormatException ex) {
            throw new IOException("Invalid file portal filter", ex);
        }
        if (selectedFilter < 0 || selectedFilter >= filters.size())
            throw new IOException("File portal returned an unknown filter");
        File[] files = new File[result.length - 1];
        try {
            for (int i = 0; i < files.length; i++) files[i] = new File(URI.create(result[i + 1]));
        } catch (IllegalArgumentException ex) {
            throw new IOException("File portal returned a non-local file", ex);
        }
        return new Selection(files, selectedFilter);
    };

    private static native String[] chooseNative(boolean save, String title, String name,
        String folder, String[] labels, String[][] patterns, int selectedFilter,
        boolean multiple, boolean directory, String acceptLabel) throws IOException;

    static List<Filter> filters(JFileChooser chooser) throws IOException {
        List<Filter> result = new ArrayList<>();
        List<FileFilter> originals = new ArrayList<>(Arrays.asList(chooser.getChoosableFileFilters()));
        if (chooser.getFileFilter() != null && !originals.contains(chooser.getFileFilter()))
            originals.add(chooser.getFileFilter());
        for (FileFilter original : originals) {
            String[] extensions;
            if (original == chooser.getAcceptAllFileFilter()) {
                result.add(new Filter(original, original.getDescription(), new String[] {"*"}, ""));
                continue;
            } else if (original instanceof FileNameExtensionFilter) {
                extensions = ((FileNameExtensionFilter) original).getExtensions();
            } else {
                // Upstream MyFileFilter exposes its suffix with getEnds().
                // https://github.com/florentbr/Owon-VDS1022/tree/a21cee14fc0807ce804657a26772784c4e47b5ef/lib
                try {
                    Method method = original.getClass().getMethod("getEnds");
                    extensions = new String[] {(String) method.invoke(original)};
                } catch (ReflectiveOperationException | ClassCastException ex) {
                    throw new IOException("Unsupported file filter: " + original.getClass().getName(), ex);
                }
            }
            if (extensions.length == 0) throw new IOException("File filter has no extensions");
            String[] patterns = new String[extensions.length];
            for (int i = 0; i < extensions.length; i++) {
                if (!extensions[i].matches("[A-Za-z0-9]+"))
                    throw new IOException("Invalid file extension: " + extensions[i]);
                StringBuilder pattern = new StringBuilder("*.");
                for (char c : extensions[i].toCharArray()) {
                    if (Character.isLetter(c)) pattern.append('[').append(Character.toLowerCase(c))
                        .append(Character.toUpperCase(c)).append(']');
                    else pattern.append(c);
                }
                patterns[i] = pattern.toString();
            }
            result.add(new Filter(original, original.getDescription(), patterns, extensions[0]));
        }
        if (result.isEmpty()) throw new IOException("File chooser has no filters");
        return result;
    }

    static String withExtension(String name, String extension) {
        if (extension.isEmpty() || name.endsWith("." + extension)) return name;
        return name + "." + extension;
    }

    static Selection waitFor(Backend backend, boolean save, String title, String name,
        String folder, List<Filter> filters, int selectedFilter, boolean multiple,
        boolean directory, String acceptLabel) throws IOException {
        if (!EventQueue.isDispatchThread())
            return backend.choose(save, title, name, folder, filters, selectedFilter,
                                  multiple, directory, acceptLabel);
        SecondaryLoop loop = Toolkit.getDefaultToolkit().getSystemEventQueue().createSecondaryLoop();
        FutureTask<Selection> task = new FutureTask<Selection>(() -> backend.choose(save,
            title, name, folder, filters, selectedFilter, multiple, directory, acceptLabel)) {
            @Override protected void done() { EventQueue.invokeLater(() -> loop.exit()); }
        };
        Thread worker = new Thread(task, "file-portal");
        worker.setDaemon(true);
        worker.start();
        if (!loop.enter()) throw new IOException("Could not wait for the file portal");
        try {
            return task.get();
        } catch (InterruptedException ex) {
            Thread.currentThread().interrupt();
            throw new IOException("File selection interrupted", ex);
        } catch (ExecutionException ex) {
            Throwable cause = ex.getCause();
            if (cause instanceof IOException) throw (IOException) cause;
            throw new IOException("File selection failed", cause);
        }
    }

    /** Recording uses a custom dialog, but upstream always appends .cap afterwards. */
    public static int recordingDialog(JFileChooser chooser, Component parent, String approveLabel) {
        chooser.setAcceptAllFileFilterUsed(false);
        chooser.setDialogType(JFileChooser.SAVE_DIALOG);
        return chooser.showDialog(parent, approveLabel);
    }
}
