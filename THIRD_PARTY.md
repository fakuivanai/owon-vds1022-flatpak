# Sources and licensing status

No license has been selected for this repository's original adapter, packaging,
test or documentation files. The AppStream metadata identifies the bundled
application as proprietary; that label is not a license for these sources.
The AppStream metadata itself declares `CC0-1.0`.

## Upstream input

The manifest downloads the archive from
[florentbr/OWON-VDS1022](https://github.com/florentbr/OWON-VDS1022/tree/a21cee14fc0807ce804657a26772784c4e47b5ef):

- Commit: `a21cee14fc0807ce804657a26772784c4e47b5ef`
- Application version: `1.1.5-cf19`
- Archive SHA-256: `730933797069bc8d084394f9ddd92052f253189ae6573c5bcc5497914752c05c`

The upstream [component notice](https://github.com/florentbr/OWON-VDS1022/blob/a21cee14fc0807ce804657a26772784c4e47b5ef/lib/copyright.md)
records these origins:

| Component | Upstream origin |
| --- | --- |
| OWON Java application and FPGA images | OWON VDS desktop installer |
| `ch.ntb.usb-0.5.9.jar` | libusbJava |
| `gson-2.7.0.jar` | Google Gson |
| `jxl-2.6.6.jar` | JExcelAPI |

The build copies the application JARs, FPGA images, icon and documentation,
patches the GUI JAR, and installs the component notice alongside the application.
It compiles an original Rust JNI bridge using the crates listed below and
links against the SDK's libusb.
It does not copy the upstream native USB libraries. OpenJDK comes from the
Freedesktop SDK extension.

## Rust dependencies

`src/native/Cargo.lock` records exact versions and checksums for direct and
transitive dependencies. `cargo-sources.json` supplies matching crate archives
for the offline Flatpak build. The direct crates declare these licenses:

| Crate | Locked version | Declared license |
| --- | --- | --- |
| [jni](https://crates.io/crates/jni/0.22.4) | `0.22.4` | MIT or Apache-2.0 |
| [rusb](https://crates.io/crates/rusb/0.9.4) | `0.9.4` | MIT |
| [zbus](https://crates.io/crates/zbus/5.19.0) | `5.19.0` | MIT |
| [tokio](https://crates.io/crates/tokio/1.53.1) | `1.53.1` | MIT |
| [futures-util](https://crates.io/crates/futures-util/0.3.34) | `0.3.34` | MIT or Apache-2.0 |
| [serde](https://crates.io/crates/serde/1.0.229) | `1.0.229` | MIT or Apache-2.0 |

These declarations apply to the dependencies, not to this repository's
original sources. Preserve required dependency notices, including those for
transitive crates, when redistributing a compiled adapter. The crate archives
retain their supplied notice files in the build's vendor directory. The build
installs those files under `/app/share/licenses/vdsportal/`.

## Before distributing a bundle

The upstream component notice identifies sources but does not state a
redistribution license for the OWON GUI or FPGA images. Redistribution terms
for the bundled icon and documentation also need verification. Preserve
third-party notices and check the terms for all bundled components before
publishing a binary release or preparing a Flathub submission.

This repository contains build instructions and adapters. Upstream archives,
JARs, firmware images and generated Flatpak bundles remain outside the checkout.
