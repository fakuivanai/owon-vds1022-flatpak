#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
    printf 'Usage: %s OUTPUT_DIRECTORY\n' "$0" >&2
    exit 2
fi

source_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
mkdir -p -- "$1"
output_dir=$(cd -- "$1" && pwd -P)
case "$output_dir/" in
    "$source_dir/"*)
        printf 'Keep build outputs outside the source repository.\n' >&2
        exit 2
        ;;
esac

app_id=io.github.fakuivan.owon-vds1022-flatpak
builder=${FLATPAK_BUILDER:-flatpak-builder}
branch=${FLATPAK_BRANCH:-master}
"$builder" --default-branch="$branch" --user --force-clean --disable-updates \
    --state-dir="$output_dir/state" --repo="$output_dir/repo" \
    "$output_dir/build" "$source_dir/$app_id.json"
flatpak build-bundle "$output_dir/repo" "$output_dir/$app_id.flatpak" \
    "$app_id" "$branch" --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo
