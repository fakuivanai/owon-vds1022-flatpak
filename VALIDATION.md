# Validation

Tested on 2026-10-02, x86_64 Linux with KDE Wayland, Flatpak 1.16.6,
USB portal v1 and FileChooser portal v4.
The portal service is 1.20.4 and the KDE backend is 6.6.6.

## Inputs

- Upstream application 1.1.5-cf19, commit
  `a21cee14fc0807ce804657a26772784c4e47b5ef`.
- Archive SHA-256
  `730933797069bc8d084394f9ddd92052f253189ae6573c5bcc5497914752c05c`.
- Platform 26.08, freedesktop-sdk-26.08.2; SDK freedesktop-sdk-26.08.1.
- OpenJDK extension `jdk-21.0.12.1+1`; libusb 1.0.30.
- flatpak-builder 1.4.4 and AppStream 1.1.2.

The manifest and patch sources define the package inputs. Build downloads,
repositories and bundles belong outside the source checkout.

## Passed checks

- The installed permission metadata contains only the X11 socket and USB query
  `vnd:5345+dev:1234`. No application overrides were present.
- The sandbox probe confirmed that a fresh host-home marker and
  `/run/host/etc/os-release` are hidden, along with blocked host
  TCP access, hidden raw USB nodes, read-only application/runtime files,
  writable private app data, and restricted session D-Bus access.
- Java adapter tests reject unsupported device IDs, invalid transfers and stale
  native handles. Chooser tests cover format selection, exact filename suffix
  permission, Unicode paths, cancellation, recording output and portal errors.
- Private D-Bus tests cover responses before and after method replies,
  cancellation, failure, malformed replies, wrong request paths, bus loss and
  portal service loss.
- Native file tests cover PNG/CSV filter matching, KDE's normalized globs,
  Unicode, multiple-file opening, cancellation, and unknown/ambiguous formats.
- Native code compiles with `-Wall -Wextra -Werror`. ShellCheck and AppStream
  metadata validation pass. The JAR patcher checks the pinned chooser layout.
- The complete `tests/run-native.sh` suite passes against the current sources
  and installed application JARs. It uses its own bus and fake portal service.

## Hardware and interactive checks

The connected scope was acquired through the USB portal. The adapter verified
interface 0, bulk IN `0x81` and OUT `0x03`, then released its descriptor-only
probe. The patched application read calibration and completed the normal
15-block FPGA transfer. The user confirmed a live trace.

The first screenshot export exposed KDE's filter normalization. After adding
the matching regression and rebuilding, the user confirmed that the screenshot
saved successfully to the Desktop through the FileChooser portal.

CSV/TXT/XLS/BIN exporters write to the selected file without sibling outputs in
the inspected bytecode. The adapters cover those formats; interactive export
was checked with PNG. No persistent firmware update or automatic calibration
was performed.

## USB grant reuse

The adapter now retains only the approved descriptor after a successful model
probe. It releases the probe's interface and libusb handle; application open
consumes the same descriptor, validates it again and creates a fresh transfer
handle. Ordinary close/reset release the grant. Device removal, failed reuse
and enumeration errors discard retained descriptors.

All 16 grant-lifetime regressions pass with real disposable file descriptors
and fake portal/libusb boundaries. They verify one acquisition request across
the probe/open sequence, distinct handles, device isolation and cleanup on
rejection, cancellation, malformed replies, failed claims, unplugging, reset
and shutdown. The full native/Java suite and the installed sandbox checks pass.

The rebuilt app was started with portal-message logging. Its startup made one
`AcquireDevices` call and one `FinishAcquireDevices` call. The user confirmed
one consent prompt followed by a live trace. The refreshed Flatpak bundle
contains this version.

## Host requirement and remaining work

A scope-specific `uaccess` rule was installed under `/run/udev/rules.d` for
testing. It is temporary and disappears after reboot. The repository includes
the rule and documents persistent host integration. The application still
requires X11, which allows interaction with other X11 clients.

The permission store contains approval for this scope. On the tested portal
versions, a new USB acquisition still prompts: the frontend forwards it to KDE
even when approval is stored. See the
[frontend dispatch](https://github.com/flatpak/xdg-desktop-portal/blob/1.20.4/src/usb.c#L1223-L1234)
and [KDE dialog](https://github.com/KDE/xdg-desktop-portal-kde/blob/v6.6.6/src/usb.cpp#L87-L102).

Local packaging is validated. The adapter and packaging code are unlicensed for
now. The bundled application, FPGA images, icon and documentation have separate
terms that still need verification before distributing binaries. See
[THIRD_PARTY.md](THIRD_PARTY.md).
