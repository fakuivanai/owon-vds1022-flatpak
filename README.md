# OWON VDS1022 Flatpak

An unofficial Flatpak package for the VDS1022 and VDS1022I USB oscilloscopes.
It runs the existing Java application with network access blocked, private
application storage, and USB and file access through desktop consent dialogs.

This is a personal packaging project. The Rust adapter was tested on x86_64
Linux with KDE, including one USB consent prompt, live acquisition and PNG
export to the Desktop. Other desktops must provide the USB portal.
See [VALIDATION.md](VALIDATION.md) for the tested versions and limits.

## Permissions

The manifest grants only:

| Permission | Purpose |
| --- | --- |
| `--socket=x11` | Display the Java desktop application |
| `--usb=vnd:5345+dev:1234` | Discover matching scopes through the USB portal |

The app has no network sharing, static host-filesystem access, raw USB device
grant, or unrestricted D-Bus access. The USB portal supplies an approved device
descriptor. File dialogs grant access to the files selected for opening or
saving. Private app data and temporary files are writable; the application and
runtime are read-only.

X11 access allows interaction with other X11 clients in the session. The
sandbox therefore has a weaker display boundary than a native Wayland app.
Local Flatpak overrides can also broaden access. Inspect them with:

```sh
flatpak info --show-permissions io.github.fakuivan.owon-vds1022-flatpak
flatpak override --user --show io.github.fakuivan.owon-vds1022-flatpak
```

## Build and install

Install Flatpak 1.15.11 or newer, `flatpak-builder`, and `appstreamcli` on the
host. The desktop must provide `org.freedesktop.portal.Usb` version 1 and the
FileChooser portal. USB access requires the USB portal and host device permission;
the package has no raw USB fallback.

On NixOS, `nix shell nixpkgs#flatpak-builder nixpkgs#appstream` supplies the
build tools. Flatpak and the desktop portals must already be enabled.

Add the Flathub remote and install the build dependencies:

```sh
flatpak --user remote-add --if-not-exists flathub \
  https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak --user install flathub org.freedesktop.Platform//26.08 \
  org.freedesktop.Sdk//26.08 org.freedesktop.Sdk.Extension.openjdk21//26.08 \
  org.freedesktop.Sdk.Extension.rust-stable//26.08
```

Run these commands from the repository root. The output directory holds
downloads, build files, a local Flatpak repository, and an installable bundle.
Keep it outside the source checkout. This example creates a fresh directory:

```sh
output_dir=$(mktemp -d)
./tools/build.sh "$output_dir"
flatpak --user install --noninteractive "$output_dir/repo" \
  io.github.fakuivan.owon-vds1022-flatpak
flatpak run io.github.fakuivan.owon-vds1022-flatpak
```

The build downloads a pinned upstream archive and the Rust crates listed in
`Cargo.lock`, checking their SHA-256 hashes. Cargo compiles the adapter offline
from those inputs. Downloads require network access on the host; the installed
app has none.
The bundle is `$output_dir/io.github.fakuivan.owon-vds1022-flatpak.flatpak`.
Review [third-party terms](THIRD_PARTY.md) before distributing a bundle.

## Host USB access

The desktop portal opens the scope as the logged-in user. Its consent dialog
cannot override host device permissions. The supplied
[udev rule](udev/70-owon-vds1022.rules) grants the active local user read-write
access to USB vendor/product `5345:1234`.

On a system with mutable udev configuration, run from the checkout root:

```sh
sudo install -Dm0644 udev/70-owon-vds1022.rules \
  /etc/udev/rules.d/70-owon-vds1022.rules
sudo udevadm control --reload
```

Then unplug and reconnect the scope.

On NixOS, import the supplied [udev module](udev/default.nix) in your system
configuration, adjusting the checkout path:

```nix
imports = [ /path/to/checkout/udev ];
```

Rebuild the configuration and reconnect the scope. The module uses
`services.udev.packages` to preserve the rule's `70-` prefix. This makes the
`uaccess` tag available before systemd applies seat ACLs.
`services.udev.extraRules` creates a later rule and is unsuitable for this tag.

## Use and troubleshoot

Start the app, select the scope in the USB consent dialog, and allow access.
Discovery and acquisition reuse the same approved descriptor, so startup
requires one request. A disconnect releases the grant; reconnecting can prompt
again. On the tested desktop, saved approval still prompts for a new request.

Use the application's normal open and export commands. The desktop chooser
grants the selected filename. If it lacks the selected format's extension,
the app asks for the final filename before saving. Settings and calibration
files stay in private Flatpak application data.

| Symptom | Check |
| --- | --- |
| USB portal is unavailable | The desktop portal backend must implement USB access; a recent Flatpak alone is insufficient. |
| Scope access fails after allowing it | Install the host udev rule and reconnect the scope. |
| USB consent returns after reconnecting | A new descriptor requires a new portal request on the tested desktop. |
| Saving asks for a filename twice | Include the selected format's extension in the first filename. |

Recent kernels can attach `usb_serial_simple` to the scope. The adapter detaches
it while claiming the interface and restores it when releasing the device.

## Development and validation

The package keeps the upstream GUI, acquisition logic and FPGA images. It
replaces the legacy JNI USB transport and patches file-dialog entry points
at build time. It does not run the upstream installer.

| Path | Contents |
| --- | --- |
| `src/java/` | USB compatibility class and Swing file-chooser adapters |
| `src/native/` | Rust JNI adapter, USB descriptor handling, libusb transfers and D-Bus portal calls |
| `tools/` | Build, JAR patching and launch scripts |
| `tests/` | Private portal mocks, lifecycle tests and installed sandbox probes |
| `data/` | Desktop entry and AppStream metadata |
| `udev/` | Host access rule and NixOS module |

After installation, `python3 tests/sandbox.py` checks the sandbox without opening
the GUI, requesting USB consent, or sending commands to the scope.
See [CONTRIBUTING.md](CONTRIBUTING.md) for adapter tests and changes to the pinned
upstream version, and [VALIDATION.md](VALIDATION.md) for hardware checks.

To install a separate branch for Rust validation:

```sh
rust_output_dir=$(mktemp -d)
FLATPAK_BRANCH=rust ./tools/build.sh "$rust_output_dir"
flatpak --user install --noninteractive "$rust_output_dir/repo" \
  io.github.fakuivan.owon-vds1022-flatpak//rust
flatpak run --branch=rust io.github.fakuivan.owon-vds1022-flatpak
```

## Licensing and upstream

The adapter and packaging code are unlicensed for now. The upstream application,
firmware, icon and documentation have separate terms; their redistribution
rights have not been verified. No upstream binaries are committed here.
See [THIRD_PARTY.md](THIRD_PARTY.md) for the pinned inputs and notices.
