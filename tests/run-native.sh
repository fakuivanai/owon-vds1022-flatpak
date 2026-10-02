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
for tool in cc pkg-config dbus-daemon dbus-run-session; do
    command -v "$tool" >/dev/null
done
pkg-config --atleast-version=1.0.27 libusb-1.0
mkdir -p -- "$output_dir"
run_dir=$(mktemp -d "$output_dir/native-tests.XXXXXX")
classes="$run_dir/classes"
mkdir -p -- "$classes"
cd -- "$source_dir"

# Word splitting is intentional: pkg-config emits compiler arguments.
# shellcheck disable=SC2046
cc -std=c11 -g -Wall -Wextra -Werror -Isrc/native \
    $(pkg-config --cflags gio-2.0) tests/PortalRequestTest.c src/native/portal.c \
    -o "$run_dir/portal-request-test" $(pkg-config --libs gio-2.0)
# shellcheck disable=SC2046
cc -std=c11 -g -Wall -Wextra -Werror -Isrc/native \
    -I"$jdk/include" -I"$jdk/include/linux" \
    $(pkg-config --cflags gio-unix-2.0 libusb-1.0) \
    tests/usb-properties-test.c src/native/portal.c \
    -o "$run_dir/usb-properties-test" $(pkg-config --libs gio-unix-2.0 libusb-1.0)
# shellcheck disable=SC2046
cc -std=c11 -g -Wall -Wextra -Werror -Isrc/native \
    -I"$jdk/include" -I"$jdk/include/linux" \
    $(pkg-config --cflags gio-unix-2.0 libusb-1.0) \
    tests/usb-grant-lifetime-test.c \
    -o "$run_dir/usb-grant-lifetime-test" $(pkg-config --libs gio-unix-2.0 libusb-1.0)
# shellcheck disable=SC2046
cc -std=c11 -g -Wall -Wextra -Werror \
    $(pkg-config --cflags gio-2.0) tests/file-portal-mock.c \
    -o "$run_dir/file-portal-mock" $(pkg-config --libs gio-2.0)
# shellcheck disable=SC2046
cc -std=c11 -g -Wall -Wextra -Werror -fPIC -shared \
    -I"$jdk/include" -I"$jdk/include/linux" \
    $(pkg-config --cflags gio-unix-2.0 libusb-1.0) \
    src/native/portal.c src/native/usb.c src/native/files.c \
    -o "$run_dir/libvdsportal.so" $(pkg-config --libs gio-unix-2.0 libusb-1.0)
# Swing's chooser is not persisted with Java serialization.
"$jdk/bin/javac" --release 17 -Xlint:all,-serial -Werror -cp "$app_lib/*" -d "$classes" \
    src/java/com/owon/uppersoft/vds/core/usb/CDevice.java \
    src/java/org/vds1022/portal/*.java tests/UsbBackendTest.java \
    tests/java/org/vds1022/portal/FilePortalNativeTest.java \
    tests/java/org/vds1022/portal/PortalFileChooserTest.java

cd -- "$run_dir"
"$run_dir/portal-request-test"
"$run_dir/usb-properties-test"
"$run_dir/usb-grant-lifetime-test"
"$jdk/bin/java" -Djava.awt.headless=true -Djava.library.path="$run_dir" \
    -cp "$classes:$app_lib/*" UsbBackendTest
"$jdk/bin/java" -Djava.awt.headless=true -Djava.library.path="$run_dir" \
    -cp "$classes:$app_lib/*" org.vds1022.portal.PortalFileChooserTest
# A private bus without system configuration or activation of desktop services.
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
    "$run_dir/file-portal-mock" "$jdk/bin/java" "$run_dir" "$classes:$app_lib/*"
printf 'Native tests passed. Outputs: %s\n' "$run_dir"
