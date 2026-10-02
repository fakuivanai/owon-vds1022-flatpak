#!/usr/bin/env python3
"""Generate the Flatpak's offline crates.io sources from Cargo.lock."""

import argparse
import json
from pathlib import Path
import tomllib

# The archive/checksum format follows flatpak-builder-tools:
# https://github.com/flatpak/flatpak-builder-tools/blob/74697c75b630d7330e77250fc13cb5ea688d9479/cargo/flatpak-cargo-generator.py
# This project accepts only crates.io inputs; other source kinds fail explicitly.
def generate(lock):
    sources = []
    for package in lock["package"]:
        if "source" not in package:
            continue
        if package["source"] != "registry+https://github.com/rust-lang/crates.io-index":
            raise ValueError(f"Unsupported Cargo source: {package['source']}")
        name, version, checksum = package["name"], package["version"], package["checksum"]
        destination = f"cargo/vendor/{name}-{version}"
        sources.extend([
            {
                "type": "archive",
                "archive-type": "tar-gzip",
                "url": f"https://static.crates.io/crates/{name}/{name}-{version}.crate",
                "sha256": checksum,
                "dest": destination,
            },
            {
                "type": "inline",
                "contents": json.dumps({"package": checksum, "files": {}}, sort_keys=True),
                "dest": destination,
                "dest-filename": ".cargo-checksum.json",
            },
        ])
    sources.append({
        "type": "inline",
        "dest": "cargo",
        "dest-filename": "config.toml",
        "contents": '[source.crates-io]\nreplace-with = "vendored-sources"\n\n'
                    '[source.vendored-sources]\ndirectory = "cargo/vendor"\n',
    })
    return json.dumps(sources, indent=2) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    expected = generate(tomllib.loads((root / "src/native/Cargo.lock").read_text()))
    output = root / "cargo-sources.json"
    if args.check:
        if output.read_text() != expected:
            parser.error("cargo-sources.json differs from Cargo.lock; run tools/cargo-sources.py")
    else:
        output.write_text(expected)


if __name__ == "__main__":
    main()
