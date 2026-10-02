#!/usr/bin/env bash
set -euo pipefail

jdk=/usr/lib/sdk/openjdk21/jvm/openjdk-21
app_dir=/app/share/owon-vds1022
app_id=io.github.fakuivan.owon-vds1022-flatpak
test -x "$jdk/bin/javac"
pkg-config --atleast-version=1.0.27 libusb-1.0

mkdir -p "$app_dir/lib" patch-classes patch-tools /app/lib
cp -r upstream/fwr upstream/doc upstream/version.txt "$app_dir/"
cp upstream/lib/*.jar "$app_dir/lib/"

"$jdk/bin/javac" --release 8 -Xlint:all \
    -cp 'upstream/lib/*' -d patch-classes \
    src/java/com/owon/uppersoft/vds/core/usb/CDevice.java \
    src/java/org/vds1022/portal/*.java
"$jdk/bin/javac" --add-exports java.base/jdk.internal.org.objectweb.asm=ALL-UNNAMED \
    -d patch-tools tools/PatchChoosers.java
"$jdk/bin/java" --add-exports java.base/jdk.internal.org.objectweb.asm=ALL-UNNAMED \
    -cp patch-tools PatchChoosers upstream/lib/owon-vds-tiny-1.1.5-cf19.jar \
    "$app_dir/lib/owon-vds-tiny-1.1.5-cf19.jar"
"$jdk/bin/jar" --update --file "$app_dir/lib/owon-vds-tiny-1.1.5-cf19.jar" \
    -C patch-classes .

cargo build --offline --locked --release --lib --manifest-path src/native/Cargo.toml
install -m755 src/native/target/release/libvdsportal.so /app/lib/libvdsportal.so
while IFS= read -r -d '' notice; do
    install -Dm644 "$notice" "/app/share/licenses/vdsportal/${notice#cargo/vendor/}"
done < <(find cargo/vendor -type f \( -iname 'license*' -o -iname 'copying*' -o -iname 'notice*' -o -iname 'copyright*' \) -print0)

mkdir -p /app/share/owon-vds1022-flatpak/tests
"$jdk/bin/javac" --release 17 -Xlint:all \
    -cp "$app_dir/lib/*" -d /app/share/owon-vds1022-flatpak/tests \
    tests/*.java tests/java/org/vds1022/portal/*.java

test_classpath="/app/share/owon-vds1022-flatpak/tests:$app_dir/lib/*"
"$jdk/bin/java" -Djava.library.path=/app/lib -cp "$test_classpath" UsbBackendTest
"$jdk/bin/java" -Djava.awt.headless=true -Djava.library.path=/app/lib \
    -cp "$test_classpath" org.vds1022.portal.PortalFileChooserTest

install -Dm755 tools/launch.sh /app/bin/owon-vds1022
install -Dm644 "data/$app_id.desktop" "/app/share/applications/$app_id.desktop"
install -Dm644 "data/$app_id.metainfo.xml" "/app/share/metainfo/$app_id.metainfo.xml"
install -Dm644 upstream/ico/icon.svg "/app/share/icons/hicolor/scalable/apps/$app_id.svg"
install -Dm644 upstream/lib/copyright.md "$app_dir/copyright.md"
