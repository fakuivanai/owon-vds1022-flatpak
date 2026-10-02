# Development

This is a personal packaging project. Keep changes focused and include the
test results relevant to the behavior you change.

## Adapter tests

The test runner needs a C compiler, `pkg-config`, GIO development files,
libusb 1.0.27 or newer, a JDK 17 or newer, `dbus-daemon`, and `dbus-run-session`.
It also needs the application JARs from the pinned upstream archive or a built
payload. Run from the checkout root:

```sh
jdk_dir=/path/to/jdk
app_lib_dir=/path/to/build-output/build/files/share/owon-vds1022/lib
test_output_dir=$(mktemp -d)
./tests/run-native.sh "$jdk_dir" "$test_output_dir" "$app_lib_dir"
```

The runner compiles the current Java and native adapters with warnings enabled.
Its private D-Bus services and fake libusb calls test request ordering,
cancellation, malformed responses, descriptor ownership, disconnects, file
filters, Unicode filenames, and cleanup. It stays headless and does not contact
the desktop portal or USB hardware. Keep generated files outside the checkout.

After installing a build, run `python3 tests/sandbox.py` to verify its permissions
and host isolation. Acquisition and interactive exports require separate
hardware checks. Record which formats and desktops were actually tested.

## Changes and upstream updates

Preserve the USB query for `5345:1234`, network isolation and portal file access.
Do not add broad filesystem, device or D-Bus access to work around an error.
Keep resource ownership and failure cleanup explicit. Add provenance comments
with pinned source links when adapting external code.

The manifest pins the upstream archive revision and checksum. When changing
them, also review the JAR filename and compatibility classes used by
`tools/install-payload.sh`. `PatchChoosers` expects eight chooser constructions
and one recording-dialog call; it fails if that layout changes. Review the
new bytecode and update the adapters deliberately before accepting a new layout.

Use ShellCheck for changed shell scripts and `appstreamcli validate --no-net`
for the AppStream metadata. Keep upstream notices intact and review
[THIRD_PARTY.md](THIRD_PARTY.md) before distributing build artifacts.
