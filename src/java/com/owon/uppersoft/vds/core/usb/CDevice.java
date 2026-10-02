package com.owon.uppersoft.vds.core.usb;

import ch.ntb.usb.USBException;
import com.owon.uppersoft.dso.source.usb.USBPortsFilter;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Iterator;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Set;

/**
 * Portal-backed replacement for the application's direct libusb transport.
 * The compatibility contract was inspected in the upstream CDevice/IDevice:
 * https://github.com/florentbr/Owon-VDS1022/blob/a21cee14fc0807ce804657a26772784c4e47b5ef/lib/owon-vds-tiny-1.1.5-cf19.jar
 * This implementation is original; it retains the upstream model filter.
 */
public final class CDevice extends ComparableDevice implements IDevice {
    public static int CONTEXT;

    private static final int VENDOR_ID = 0x5345;
    private static final int PRODUCT_ID = 0x1234;
    private static final Map<String, CDevice> DEVICES = new HashMap<>();
    private static final Set<String> DECLINED = new HashSet<>();

    static {
        System.loadLibrary("vdsportal");
    }

    private final String portalId;
    private long handle;

    private CDevice(String portalId) {
        this.portalId = Objects.requireNonNull(portalId);
        // The upstream filter fills this with the model after its read-only query.
        serialNumber = "";
    }

    /** A cancelled portal prompt must not reopen on every discovery poll. */
    private static final class AccessCancelled extends USBException {
        private static final long serialVersionUID = 1L;

        private AccessCancelled(String message) {
            super(message);
        }
    }

    public static synchronized void init() {
        initNative();
    }

    public static synchronized void release() {
        try {
            releaseNative();
        } finally {
            for (CDevice device : DEVICES.values()) {
                device.handle = 0;
            }
            DEVICES.clear();
            DECLINED.clear();
            CONTEXT++;
        }
    }

    public static synchronized List<IDevice> getDevices(
            short vendor, short product, USBPortsFilter filter) throws USBException {
        if (Short.toUnsignedInt(vendor) != VENDOR_ID
                || Short.toUnsignedInt(product) != PRODUCT_ID) {
            return new ArrayList<>();
        }

        Set<String> present = new HashSet<>(Arrays.asList(enumerateNative()));
        DECLINED.retainAll(present);
        Iterator<Map.Entry<String, CDevice>> iterator = DEVICES.entrySet().iterator();
        while (iterator.hasNext()) {
            Map.Entry<String, CDevice> entry = iterator.next();
            if (!present.contains(entry.getKey())) {
                iterator.remove();
                entry.getValue().close();
            }
        }

        List<IDevice> devices = new ArrayList<>();
        for (String id : present) {
            CDevice device = DEVICES.get(id);
            if (device != null) {
                devices.add(device);
                continue;
            }
            if (DECLINED.contains(id)) {
                continue;
            }
            device = new CDevice(id);
            boolean accepted = false;
            try {
                device.open();
                accepted = filter == null || filter.collectPort(device);
                if (!accepted) {
                    DECLINED.add(id);
                }
            } catch (AccessCancelled cancelled) {
                DECLINED.add(id);
            } finally {
                if (accepted) {
                    device.closeProbe();
                } else {
                    device.close();
                }
            }
            if (accepted) {
                DEVICES.put(id, device);
                devices.add(device);
            }
        }
        return devices;
    }

    /** Keep only the accepted discovery grant for the application's next open. */
    private void closeProbe() throws USBException {
        synchronized (CDevice.class) {
            if (handle == 0) {
                throw new USBException("Discovery probe is already closed");
            }
            long closing = handle;
            handle = 0;
            closeProbeNative(closing);
        }
    }

    @Override
    public boolean equals(IDevice other) {
        return other instanceof CDevice && portalId.equals(((CDevice) other).portalId);
    }

    @Override
    public boolean isOpen() {
        synchronized (CDevice.class) {
            return handle != 0;
        }
    }

    @Override
    public void open() throws USBException {
        synchronized (CDevice.class) {
            if (handle != 0) {
                throw new USBException("Device is already open");
            }
            int[] metadata = new int[8];
            handle = openNative(portalId, metadata);
            bConfigurationValue = metadata[0];
            bInterfaceNumber = metadata[1];
            bAlternateSetting = metadata[2];
            ReadEndpoint = metadata[3];
            WriteEndpoint = metadata[4];
            RdEpMaxPacketSize = metadata[5];
            WrEpMaxPacketSize = metadata[6];
            bcdUSB = metadata[7];
        }
    }

    @Override
    public void close() throws USBException {
        synchronized (CDevice.class) {
            if (handle != 0) {
                long closing = handle;
                handle = 0;
                closeNative(closing);
            } else {
                forgetNative(portalId);
            }
        }
    }

    @Override
    public void reset() throws USBException {
        synchronized (CDevice.class) {
            if (handle != 0) {
                long resetting = handle;
                handle = 0;
                resetNative(resetting);
            } else {
                forgetNative(portalId);
            }
        }
    }

    private int transfer(byte[] data, int size, int timeout, boolean read) throws USBException {
        Objects.requireNonNull(data, "data");
        if (size <= 0 || size > data.length || timeout <= 0) {
            throw new IllegalArgumentException("Invalid USB transfer size or timeout");
        }
        synchronized (CDevice.class) {
            if (handle == 0) {
                throw new USBException("Device is closed");
            }
            return transferNative(handle, data, size, timeout, read);
        }
    }

    @Override
    public int readBulk(byte[] data, int size, int timeout) throws USBException {
        return transfer(data, size, timeout, true);
    }

    @Override
    public int writeBulk(byte[] data, int size, int timeout) throws USBException {
        return transfer(data, size, timeout, false);
    }

    private static native void initNative();
    private static native void releaseNative();
    private static native String[] enumerateNative() throws USBException;
    private static native long openNative(String id, int[] metadata) throws USBException;
    private static native void closeNative(long handle) throws USBException;
    private static native void closeProbeNative(long handle) throws USBException;
    private static native void forgetNative(String id) throws USBException;
    private static native void resetNative(long handle) throws USBException;
    private static native int transferNative(
            long handle, byte[] data, int size, int timeout, boolean read) throws USBException;
}
