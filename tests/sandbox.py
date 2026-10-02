#!/usr/bin/env python3
"""Check an installed Flatpak without opening the application or consent dialogs."""

import argparse
import configparser
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import xml.etree.ElementTree as ET


APP_ID = "io.github.fakuivan.owon-vds1022-flatpak"
PROBE_CLASSES = "/app/share/owon-vds1022-flatpak/tests"


def run(*arguments, check=True):
    return subprocess.run(arguments, check=check, capture_output=True, text=True, timeout=30)


def permission_values(configuration, section, key):
    value = configuration.get(section, key, fallback="")
    return set(filter(None, value.split(";")))


def check_permissions(app_id):
    metadata = configparser.ConfigParser(interpolation=None)
    metadata.read_string(run("flatpak", "info", "--show-permissions", app_id).stdout)
    if "network" in permission_values(metadata, "Context", "shared"):
        raise AssertionError("Installed application requests network sharing")
    if permission_values(metadata, "Context", "filesystems"):
        raise AssertionError("Installed application requests static host filesystem access")
    if permission_values(metadata, "Context", "devices") - {"!all"}:
        raise AssertionError("Installed application requests direct device access")
    sockets = permission_values(metadata, "Context", "sockets")
    if {"session-bus", "system-bus"} & sockets:
        raise AssertionError("Installed application requests an unfiltered D-Bus socket")
    for section in ("Session Bus Policy", "System Bus Policy"):
        if not metadata.has_section(section):
            continue
        for name, access in metadata.items(section):
            if access != "none" and not name.startswith("org.freedesktop.portal."):
                raise AssertionError(f"Unexpected host D-Bus grant: {name}={access}")
    usb = permission_values(metadata, "USB Devices", "enumerable-devices")
    if usb != {"vnd:5345+dev:1234"}:
        raise AssertionError(f"Unexpected USB enumeration policy: {usb}")
    print("PASS installed permissions restrict USB enumeration to the scope")


def check_portal(app_id):
    result = run(
        "flatpak", "run", "--command=gdbus", app_id,
        "introspect", "--session", "--dest=org.freedesktop.portal.Desktop",
        "--object-path=/org/freedesktop/portal/desktop", "--xml",
    )
    document = ET.fromstring(result.stdout)
    interfaces = {element.attrib["name"] for element in document.findall("interface")}
    required = {"org.freedesktop.portal.Usb", "org.freedesktop.portal.FileChooser"}
    if not required <= interfaces:
        raise AssertionError(f"Missing required portals: {required - interfaces}")
    forbidden = run(
        "flatpak", "run", "--command=gdbus", app_id,
        "introspect", "--session", "--dest=org.freedesktop.systemd1",
        "--object-path=/org/freedesktop/systemd1", "--xml", check=False,
    )
    if forbidden.returncode == 0:
        raise AssertionError("Host session systemd service is accessible")
    print("PASS USB and FileChooser portals are available and session systemd is inaccessible")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("app_id", nargs="?", default=APP_ID)
    arguments = parser.parse_args()
    check_permissions(arguments.app_id)
    check_portal(arguments.app_id)
    namespace = os.readlink("/proc/self/ns/net")
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        port = listener.getsockname()[1]
        # A fresh marker checks real host-home visibility without reading user files.
        with tempfile.NamedTemporaryFile(prefix="sandbox-check-", dir=Path.home()) as marker:
            result = run(
                "flatpak", "run", "--command=/app/jre/bin/java", arguments.app_id,
                "-cp", PROBE_CLASSES, "SandboxProbe", marker.name, namespace, str(port),
            )
            print(result.stdout, end="")


if __name__ == "__main__":
    main()
