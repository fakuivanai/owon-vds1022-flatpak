import com.owon.uppersoft.vds.core.usb.CDevice;
import java.lang.reflect.Constructor;
import java.lang.reflect.Method;

/** Explicit descriptor-only probe. Does not invoke the application's model/firmware logic. */
public final class UsbProbe {
    public static void main(String[] arguments) throws Exception {
        boolean acquire = arguments.length == 1 && "--acquire-descriptors".equals(arguments[0]);
        if (arguments.length != 0 && !acquire) {
            throw new IllegalArgumentException("Usage: UsbProbe [--acquire-descriptors]");
        }
        CDevice.init();
        try {
            Method enumerate = CDevice.class.getDeclaredMethod("enumerateNative");
            enumerate.setAccessible(true);
            String[] ids = (String[]) enumerate.invoke(null);
            System.out.println("Matching scope portal IDs: " + ids.length);
            if (acquire) {
                Constructor<CDevice> constructor = CDevice.class.getDeclaredConstructor(String.class);
                constructor.setAccessible(true);
                for (String id : ids) {
                    CDevice device = constructor.newInstance(id);
                    device.open();
                    try {
                        System.out.printf("Interface %d, IN 0x%02x, OUT 0x%02x, USB 0x%04x%n",
                                device.getBInterfaceNumber(), device.getReadEndpoint(),
                                device.getWriteEndpoint(), device.getBcdUSB());
                    } finally {
                        device.close();
                    }
                }
            }
        } finally {
            CDevice.release();
        }
    }
}
