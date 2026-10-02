import java.io.IOException;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.jar.JarEntry;
import java.util.jar.JarFile;
import java.util.jar.JarOutputStream;
import java.nio.file.Files;
import jdk.internal.org.objectweb.asm.ClassReader;
import jdk.internal.org.objectweb.asm.ClassVisitor;
import jdk.internal.org.objectweb.asm.ClassWriter;
import jdk.internal.org.objectweb.asm.MethodVisitor;
import jdk.internal.org.objectweb.asm.Opcodes;

/** Build-time use of the ASM bundled with the JDK. No application decompilation. */
public final class PatchChoosers {
    private static final String SWING = "javax/swing/JFileChooser";
    private static final String PORTAL = "org/vds1022/portal/PortalFileChooser";
    private static int constructions;
    private static int constructors;
    private static int recordingDialogs;

    private static byte[] patch(byte[] input) {
        ClassReader reader = new ClassReader(input);
        if (SWING.equals(reader.getSuperName()))
            throw new IllegalArgumentException("Upstream introduced a custom JFileChooser subclass");
        ClassWriter writer = new ClassWriter(reader, 0);
        reader.accept(new ClassVisitor(Opcodes.ASM8, writer) {
            @Override public MethodVisitor visitMethod(int access, String name, String descriptor,
                String signature, String[] exceptions) {
                return new MethodVisitor(Opcodes.ASM8,
                    super.visitMethod(access, name, descriptor, signature, exceptions)) {
                    @Override public void visitTypeInsn(int opcode, String type) {
                        if (opcode == Opcodes.NEW && type.equals(SWING)) {
                            constructions++;
                            type = PORTAL;
                        }
                        super.visitTypeInsn(opcode, type);
                    }
                    @Override public void visitMethodInsn(int opcode, String owner, String method,
                        String desc, boolean isInterface) {
                        if (opcode == Opcodes.INVOKESPECIAL && owner.equals(SWING) && method.equals("<init>")) {
                            constructors++;
                            owner = PORTAL;
                        }
                        if (reader.getClassName().equals("com/owon/uppersoft/dso/view/pane/function/RecordPane")
                            && name.equals("browseforSaveas") && owner.equals(SWING)
                            && method.equals("showDialog")
                            && desc.equals("(Ljava/awt/Component;Ljava/lang/String;)I")) {
                            recordingDialogs++;
                            super.visitMethodInsn(Opcodes.INVOKESTATIC, "org/vds1022/portal/FilePortal",
                                "recordingDialog", "(Ljavax/swing/JFileChooser;Ljava/awt/Component;Ljava/lang/String;)I", false);
                            return;
                        }
                        super.visitMethodInsn(opcode, owner, method, desc, isInterface);
                    }
                };
            }
        }, 0);
        return writer.toByteArray();
    }

    public static void main(String[] args) throws IOException {
        if (args.length != 2) throw new IllegalArgumentException("Usage: PatchChoosers input.jar output.jar");
        List<String> changed = new ArrayList<>();
        try (JarFile input = new JarFile(args[0]);
             JarOutputStream output = new JarOutputStream(Files.newOutputStream(Path.of(args[1])))) {
            for (JarEntry entry : java.util.Collections.list(input.entries())) {
                if (entry.getName().matches("META-INF/[^/]+\\.(SF|RSA|DSA|EC)"))
                    throw new IOException("Refusing to modify a signed JAR");
                byte[] data;
                try (java.io.InputStream stream = input.getInputStream(entry)) { data = stream.readAllBytes(); }
                if (entry.getName().endsWith(".class")) {
                    int before = constructions;
                    data = patch(data);
                    if (constructions != before) changed.add(entry.getName());
                }
                JarEntry copy = new JarEntry(entry.getName());
                copy.setTime(0L);
                output.putNextEntry(copy);
                output.write(data);
                output.closeEntry();
            }
        }
        if (constructions != 8 || constructors != 8 || recordingDialogs != 1)
            throw new IOException("Upstream chooser layout changed: new=" + constructions
                + ", init=" + constructors + ", recording=" + recordingDialogs);
        System.out.println("Patched " + constructions + " chooser constructions in " + changed.size() + " classes.");
    }
}
