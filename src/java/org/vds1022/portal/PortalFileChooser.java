package org.vds1022.portal;

import java.awt.Component;
import java.awt.GraphicsEnvironment;
import java.awt.Window;
import java.io.File;
import java.io.IOException;
import java.util.List;
import javax.swing.JFileChooser;
import javax.swing.JOptionPane;
import javax.swing.SwingUtilities;
import javax.swing.UIManager;
import javax.swing.filechooser.FileFilter;
import javax.swing.filechooser.FileSystemView;
import javax.swing.filechooser.FileView;
import javax.swing.plaf.FileChooserUI;

/** Replaces only upstream JFileChooser construction; export code stays intact. */
public final class PortalFileChooser extends JFileChooser {
    private static final FileFilter ALL_FILES = new FileFilter() {
        @Override public boolean accept(File file) { return true; }
        @Override public String getDescription() {
            String label = UIManager.getString("FileChooser.acceptAllFileFilterText");
            return label == null ? "All files" : label;
        }
    };
    private static final FileChooserUI PORTAL_UI = new FileChooserUI() {
        @Override public FileFilter getAcceptAllFileFilter(JFileChooser chooser) { return ALL_FILES; }
        @Override public FileView getFileView(JFileChooser chooser) { return null; }
        @Override public String getApproveButtonText(JFileChooser chooser) {
            return chooser.getDialogType() == SAVE_DIALOG ? "Save" : "Open";
        }
        @Override public String getDialogTitle(JFileChooser chooser) {
            return chooser.getDialogType() == SAVE_DIALOG ? "Save file" : "Open file";
        }
        @Override public void rescanCurrentDirectory(JFileChooser chooser) {}
        @Override public void ensureFileIsVisible(JFileChooser chooser, File file) {}
    };
    private final FilePortal.Backend backend;

    public PortalFileChooser() { super(); backend = FilePortal.NATIVE; }
    public PortalFileChooser(File directory) { super(directory); backend = FilePortal.NATIVE; }
    public PortalFileChooser(String directory) { super(directory); backend = FilePortal.NATIVE; }
    public PortalFileChooser(FileSystemView view) { super(view); backend = FilePortal.NATIVE; }
    public PortalFileChooser(File directory, FileSystemView view) { super(directory, view); backend = FilePortal.NATIVE; }
    public PortalFileChooser(String directory, FileSystemView view) { super(directory, view); backend = FilePortal.NATIVE; }
    PortalFileChooser(FilePortal.Backend backend) { super(); this.backend = backend; }

    @Override public void updateUI() {
        // The portal owns the browser UI; do not construct a hidden local file browser.
        setUI(PORTAL_UI);
        if (isAcceptAllFileFilterUsed()) addChoosableFileFilter(ALL_FILES);
    }

    @Override public int showOpenDialog(Component parent) {
        setDialogType(OPEN_DIALOG);
        return showPortal(parent, false);
    }

    @Override public int showSaveDialog(Component parent) {
        setDialogType(SAVE_DIALOG);
        return showPortal(parent, true);
    }

    @Override public int showDialog(Component parent, String approveLabel) {
        setApproveButtonText(approveLabel);
        return showPortal(parent, getDialogType() == SAVE_DIALOG);
    }

    private int showPortal(Component parent, boolean save) {
        Window owner = parent == null ? null : SwingUtilities.getWindowAncestor(parent);
        if (parent instanceof Window) owner = (Window) parent;
        boolean enabled = owner != null && owner.isEnabled();
        if (enabled) owner.setEnabled(false);
        try {
            if (getAccessory() != null) throw new IOException("Custom chooser accessories are not supported");
            if (save && getFileSelectionMode() != FILES_ONLY)
                throw new IOException("Saving directories is not supported");
            List<FilePortal.Filter> filters = FilePortal.filters(this);
            int selectedFilter = 0;
            for (int i = 0; i < filters.size(); i++)
                if (filters.get(i).original == getFileFilter()) selectedFilter = i;
            String title = getDialogTitle();
            if (title == null || title.isEmpty()) title = save ? "Save file" : "Open file";
            String name = getSelectedFile() == null ? "capture" : getSelectedFile().getName();
            String folder = getCurrentDirectory() == null ? "" : getCurrentDirectory().getAbsolutePath();
            if (save) name = FilePortal.withExtension(name, filters.get(selectedFilter).extension);
            while (true) {
                FilePortal.Selection selection = FilePortal.waitFor(backend, save, title, name,
                    folder, filters, selectedFilter, isMultiSelectionEnabled() && !save,
                    getFileSelectionMode() == DIRECTORIES_ONLY, getApproveButtonText());
                if (selection == null) return CANCEL_OPTION;
                if (selection.filter < 0 || selection.filter >= filters.size())
                    throw new IOException("File portal returned an unknown format");
                if (save && selection.files.length != 1)
                    throw new IOException("File portal returned more than one save destination");
                File selected = selection.files[0];
                String extension = filters.get(selection.filter).extension;
                if (save && !selected.getName().equals(FilePortal.withExtension(selected.getName(), extension))) {
                    // The host grants the selected file, not a sibling with an appended suffix.
                    // Request the final filename before upstream's suffix logic can change it.
                    name = FilePortal.withExtension(selected.getName(), extension);
                    folder = selected.getParent();
                    selectedFilter = selection.filter;
                    continue;
                }
                setFileFilter(filters.get(selection.filter).original);
                setSelectedFile(selected);
                if (isMultiSelectionEnabled()) setSelectedFiles(selection.files);
                setCurrentDirectory(getFileSelectionMode() == DIRECTORIES_ONLY ? selected : selected.getParentFile());
                return APPROVE_OPTION;
            }
        } catch (IOException | LinkageError ex) {
            if (!GraphicsEnvironment.isHeadless())
                JOptionPane.showMessageDialog(parent, ex.getMessage(), "File selection failed", JOptionPane.ERROR_MESSAGE);
            else System.err.println("File selection failed: " + ex.getMessage());
            return ERROR_OPTION;
        } finally {
            if (enabled) owner.setEnabled(true);
        }
    }
}
