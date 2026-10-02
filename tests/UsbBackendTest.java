import ch.ntb.usb.USBException;
import com.owon.uppersoft.vds.core.usb.CDevice;
import java.lang.reflect.Constructor;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;

/** Runs without contacting a portal, acquiring USB access, or sending device commands. */
public final class UsbBackendTest {
    private static void rejects(Method method, Class<? extends Throwable> expected, Object... arguments)
            throws Exception {
        try {
            method.invoke(null, arguments);
            throw new AssertionError(method.getName() + " accepted an invalid operation");
        } catch (InvocationTargetException failure) {
            if (!expected.isInstance(failure.getCause())) {
                throw new AssertionError("Expected " + expected.getName(), failure.getCause());
            }
        }
    }

    public static void main(String[] arguments) throws Exception {
        CDevice.init();
        try {
            if (!CDevice.getDevices((short) 0x1234, (short) 0x1234, null).isEmpty()) {
                throw new AssertionError("Unsupported vendor was accepted");
            }
            if (!CDevice.getDevices((short) 0x5345, (short) 0x5678, null).isEmpty()) {
                throw new AssertionError("Unsupported product was accepted");
            }

            Method transfer = CDevice.class.getDeclaredMethod("transferNative", long.class,
                    byte[].class, int.class, int.class, boolean.class);
            transfer.setAccessible(true);
            rejects(transfer, IllegalArgumentException.class, 1L, null, 1, 200, true);
            rejects(transfer, IllegalArgumentException.class, 1L, new byte[1], 2, 200, true);
            rejects(transfer, IllegalArgumentException.class, 1L, new byte[1], 1, 0, true);
            rejects(transfer, USBException.class, Long.MAX_VALUE, new byte[1], 1, 200, true);

            Method open = CDevice.class.getDeclaredMethod("openNative", String.class, int[].class);
            open.setAccessible(true);
            rejects(open, IllegalArgumentException.class, null, new int[8]);
            rejects(open, IllegalArgumentException.class, "untrusted-id", new int[7]);

            Method close = CDevice.class.getDeclaredMethod("closeNative", long.class);
            close.setAccessible(true);
            rejects(close, USBException.class, Long.MAX_VALUE);

            Method closeProbe = CDevice.class.getDeclaredMethod("closeProbeNative", long.class);
            closeProbe.setAccessible(true);
            rejects(closeProbe, USBException.class, Long.MAX_VALUE);

            Method forget = CDevice.class.getDeclaredMethod("forgetNative", String.class);
            forget.setAccessible(true);
            rejects(forget, IllegalArgumentException.class, (Object) null);
            forget.invoke(null, "not-a-granted-device");
            forget.invoke(null, "not-a-granted-device");

            Constructor<CDevice> constructor = CDevice.class.getDeclaredConstructor(String.class);
            constructor.setAccessible(true);
            CDevice device = constructor.newInstance("not-a-granted-device");
            if (device.isOpen()) {
                throw new AssertionError("New device reports itself open");
            }
            device.close();
            device.reset();
            try {
                device.writeBulk(new byte[1], 1, 200);
                throw new AssertionError("Closed device accepted a transfer");
            } catch (USBException expected) {
                // A closed device must fail before native I/O or portal acquisition.
            }
            System.out.println("USB backend rejects unsupported devices, invalid buffers and stale handles");
        } finally {
            CDevice.release();
        }
    }
}
