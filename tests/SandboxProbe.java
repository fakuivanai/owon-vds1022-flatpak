import java.io.IOException;
import java.net.InetSocketAddress;
import java.net.NetworkInterface;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Collections;
import java.util.UUID;

/** Read-only host checks plus a temporary write to this application's own data. */
public final class SandboxProbe {
    private SandboxProbe() {}

    public static void main(String[] arguments) throws Exception {
        if (arguments.length != 3) {
            throw new IllegalArgumentException("Expected home marker, host network namespace, and host TCP port");
        }
        Path homeMarker = Path.of(arguments[0]);
        require(!Files.exists(homeMarker), "Host home marker is visible");
        require(!Files.exists(Path.of("/run/host/root").resolve(homeMarker.toString().substring(1))),
                "Host home marker is visible through host-root");
        require(!Files.exists(Path.of("/run/host/etc/os-release")), "Host /etc is exposed");
        System.out.println("PASS host home and /etc are hidden");

        String namespace = Files.readSymbolicLink(Path.of("/proc/self/ns/net")).toString();
        require(!namespace.equals(arguments[1]), "Host network namespace is shared");
        for (NetworkInterface network : Collections.list(NetworkInterface.getNetworkInterfaces())) {
            require(network.isLoopback(), "Non-loopback network interface is available: " + network.getName());
        }
        try (Socket connection = new Socket()) {
            try {
                connection.connect(new InetSocketAddress("127.0.0.1", Integer.parseInt(arguments[2])), 2000);
            } catch (IOException denied) {
                System.out.println("PASS connection to host TCP listener is blocked");
                verifyFiles();
                return;
            }
            throw new IllegalStateException("Connected to the host TCP listener");
        }
    }

    private static void verifyFiles() throws IOException {
        Path usb = Path.of("/dev/bus/usb");
        if (Files.exists(usb)) {
            try (var entries = Files.walk(usb)) {
                require(entries.allMatch(Files::isDirectory), "Raw USB device node is visible");
            }
        }
        System.out.println("PASS raw USB device nodes are hidden");
        verifyReadOnly(Path.of("/app"));
        verifyReadOnly(Path.of("/usr"));
        System.out.println("PASS application and runtime writes are denied");

        String dataHome = System.getenv("XDG_DATA_HOME");
        require(dataHome != null && !dataHome.isBlank(), "XDG_DATA_HOME is missing");
        Path data = Path.of(dataHome);
        Files.createDirectories(data);
        Path state = Files.createTempFile(data, "sandbox-check-", ".txt");
        try {
            Files.writeString(state, "isolated application state", StandardCharsets.UTF_8);
            require(Files.readString(state).equals("isolated application state"), "Application state readback failed");
        } finally {
            Files.deleteIfExists(state);
        }
        System.out.println("PASS application data is writable");
    }

    private static void verifyReadOnly(Path directory) throws IOException {
        Path candidate = directory.resolve("sandbox-check-" + UUID.randomUUID());
        try {
            Files.createFile(candidate);
        } catch (IOException denied) {
            return;
        }
        Files.delete(candidate);
        throw new IllegalStateException("Unexpected writable directory: " + directory);
    }

    private static void require(boolean condition, String message) {
        if (!condition) {
            throw new IllegalStateException(message);
        }
    }
}
