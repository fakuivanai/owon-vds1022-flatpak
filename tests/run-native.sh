#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
    printf 'Usage: %s JDK_DIRECTORY OUTPUT_DIRECTORY APPLICATION_LIB_DIRECTORY\n' "$0" >&2
    exit 2
fi

source_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
jdk=$(realpath -e -- "$1")
output_dir=$(realpath -m -- "$2")
app_lib=$(realpath -e -- "$3")
case "$output_dir/" in
    "$source_dir/"*)
        printf 'Keep test outputs outside the source repository.\n' >&2
        exit 2
        ;;
esac
test -x "$jdk/bin/java"
test -x "$jdk/bin/javac"
test -d "$app_lib"
for tool in cargo pkg-config dbus-daemon dbus-run-session; do
    command -v "$tool" >/dev/null
done
pkg-config --atleast-version=1.0.27 libusb-1.0
mkdir -p -- "$output_dir"
run_dir=$(mktemp -d "$output_dir/native-tests.XXXXXX")
classes="$run_dir/classes"
mkdir -p -- "$classes"
cd -- "$source_dir"

export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-"$run_dir/target"}
case "$(realpath -m -- "$CARGO_TARGET_DIR")/" in
    "$source_dir/"*) printf 'Keep Cargo outputs outside the source repository.\n' >&2; exit 2 ;;
esac
cargo test --offline --locked --all-targets --manifest-path src/native/Cargo.toml
cargo build --offline --locked --lib --bins --manifest-path src/native/Cargo.toml
native_dir="$CARGO_TARGET_DIR/debug"

# Swing's chooser is not persisted with Java serialization.
"$jdk/bin/javac" --release 17 -Xlint:all,-serial -Werror -cp "$app_lib/*" -d "$classes" \
    src/java/com/owon/uppersoft/vds/core/usb/CDevice.java \
    src/java/org/vds1022/portal/*.java tests/UsbBackendTest.java \
    tests/java/org/vds1022/portal/FilePortalNativeTest.java \
    tests/java/org/vds1022/portal/PortalFileChooserTest.java

"$jdk/bin/java" -Djava.awt.headless=true -Djava.library.path="$native_dir" \
    -cp "$classes:$app_lib/*" UsbBackendTest
"$jdk/bin/java" -Djava.awt.headless=true -Djava.library.path="$native_dir" \
    -cp "$classes:$app_lib/*" org.vds1022.portal.PortalFileChooserTest
# The private bus has no desktop activation configuration or hardware access.
cat > "$run_dir/session.conf" <<'CONFIG'
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*"/>
    <allow receive_sender="*"/>
    <allow own="*"/>
  </policy>
</busconfig>
CONFIG
dbus-run-session --config-file "$run_dir/session.conf" -- \
    env FILE_PORTAL_PRIVATE_BUS=1 bash "$source_dir/tests/run-file-portal-native.sh" \
    "$native_dir/file-portal-mock" "$jdk/bin/java" "$native_dir" "$classes:$app_lib/*"
printf 'Native tests passed. Outputs: %s\n' "$run_dir"
