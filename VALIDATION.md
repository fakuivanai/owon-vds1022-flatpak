# Validation

The Rust adapter was tested on 2026-10-02, x86_64 Linux with KDE Wayland,
Flatpak 1.16.6, USB portal v1 and FileChooser portal v4.
The portal service is 1.20.4 and the KDE backend is 6.6.6.

## Inputs

- Upstream application 1.1.5-cf19, commit
  `a21cee14fc0807ce804657a26772784c4e47b5ef`.
- Archive SHA-256
  `730933797069bc8d084394f9ddd92052f253189ae6573c5bcc5497914752c05c`.
- Platform 26.08, freedesktop-sdk-26.08.2; SDK freedesktop-sdk-26.08.1.
- OpenJDK extension `jdk-21.0.12.1+1`; libusb 1.0.30.
- Rust SDK extension 26.08, rustc and Cargo 1.98.0, extension commit
  `14774cebefeaa23a41f088a9f8ad61e98c1a32f605a3aee655744d59fe414ebc`.
- Rust dependencies pinned by `src/native/Cargo.lock`, with archive checksums
  and offline vendor configuration in `cargo-sources.json`.
- flatpak-builder 1.4.4 and AppStream 1.1.2.

The manifest, lockfile and patch sources define the package inputs.
Build downloads, repositories and bundles belong outside the source checkout.
The complete release build, export and bundle creation passed with
`FLATPAK_BRANCH=rust ./tools/build.sh <output-directory>`.
The installed test ref is
`app/io.github.fakuivan.owon-vds1022-flatpak/x86_64/rust`, at Flatpak commit
`396cf154b89bbea4d35d6bdfe6322f20ddd235544aea95a41f6600b1f96a2759`.

## Automated checks

- All 42 Rust tests passed: 28 unit tests and 14 private D-Bus integration tests.
  USB tests cover grant reuse, separate probe and transfer handles, device
  isolation, invalid buffers, stale handles, failed claims and cleanup.
- Portal tests cover responses before and after method replies, cancellation,
  failure, malformed replies, wrong request paths, unrelated response senders,
  bus loss, service loss and direct service-owner replacement.
- Java adapter tests reject unsupported device IDs, invalid transfers and stale
  native handles. Chooser tests cover format selection, exact filename suffix
  permission, Unicode paths, cancellation, recording output and portal errors.
- Native file JNI tests passed seven portal scenarios and ten malformed argument
  cases. These cover PNG/CSV filters, KDE's normalized globs, Unicode,
  multiple-file opening, cancellation, unknown/ambiguous formats, embedded NUL,
  unpaired UTF-16, invalid filter indices and null pattern arrays.
- The complete `tests/run-native.sh` suite passed with the pinned application
  JARs. It uses private buses and fake portal/USB boundaries without contacting
  the desktop portal or hardware.
- `cargo fmt --check`, Clippy with `-D warnings`, ShellCheck, AppStream validation
  and `tools/cargo-sources.py --check` passed. Cargo builds and tests used
  `--offline --locked`. The JAR patcher accepted the pinned chooser layout.

## Installed sandbox checks

The installed permission metadata contains only the X11 socket and USB query
`vnd:5345+dev:1234`. No application overrides were present.
The sandbox probe passed all checks:

- Host home and host `/etc` are hidden.
- A connection to a host TCP listener is blocked.
- Raw USB device nodes are hidden.
- Application and runtime files are read-only.
- Private application data is writable.
- The USB and FileChooser portals are available, while session systemd is
  inaccessible through the restricted D-Bus connection.

## Hardware and interactive checks

The application acquired the connected scope through the USB portal. Startup
made one `AcquireDevices` call and one `FinishAcquireDevices` call.
The probe released its interface and libusb handle while retaining the approved
file descriptor for acquisition. The acquisition opened a fresh libusb handle
with that descriptor.

The application read the scope's existing calibration and completed its normal
15-block FPGA transfer. The user confirmed one consent prompt, a live trace
and a screenshot saved to the Desktop through the FileChooser portal.

Interactive export was checked with PNG. CSV/TXT/XLS/BIN exporters use the same
chooser adapter and write only the selected file in the inspected bytecode;
those formats still require interactive checks. No persistent firmware update
or automatic calibration was performed.

## Implementation size

Physical source-line counts include comments and blank lines. Production Rust
excludes test modules and the mock portal executable.

| Code | Lines |
| --- | ---: |
| Production Rust adapter | 1,684 |
| Java compatibility adapters | 518 |
| Rust tests and portal fixture | 1,630 |
| Previous production C and headers | 1,135 |

The production Rust adapter has one executable `unsafe` block, containing the
libusb call that wraps the portal's file descriptor. The adapter owns that
descriptor until the libusb handle is dropped. Its ten JNI exports also use
`#[unsafe(no_mangle)]` for their required symbol names. No C source or header
remains in this repository. External libusb and dependency internals retain
their own C or unsafe code.

## Host requirements and limits

USB consent requires host access to the scope. The repository supplies a
scope-specific `uaccess` rule and documents persistent host integration in
[README.md](README.md). The application still requires X11, which allows
interaction with other X11 clients.

On the tested portal versions, a new USB acquisition still prompts even when
approval is stored. The frontend forwards the request to KDE's dialog.
See the
[frontend dispatch](https://github.com/flatpak/xdg-desktop-portal/blob/1.20.4/src/usb.c#L1223-L1234)
and [KDE dialog](https://github.com/KDE/xdg-desktop-portal-kde/blob/v6.6.6/src/usb.cpp#L87-L102).

The adapter and packaging code are unlicensed for now. The bundled application,
FPGA images, icon and documentation have separate terms that need verification
before distributing binaries. See [THIRD_PARTY.md](THIRD_PARTY.md).
