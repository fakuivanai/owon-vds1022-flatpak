#!/usr/bin/env bash
set -euo pipefail

if [[ $# != 4 ]]; then
    echo "Usage: $0 mock-executable java-executable native-library-directory classpath" >&2
    exit 2
fi
mock_executable=$1
java_executable=$2
native_directory=$3
test_classpath=$4

if [[ ${FILE_PORTAL_PRIVATE_BUS:-} != 1 ]]; then
    exec dbus-run-session -- env FILE_PORTAL_PRIVATE_BUS=1 "$0" "$@"
fi

coproc FILE_PORTAL_MOCK { exec "$mock_executable"; }
mock_pid=$FILE_PORTAL_MOCK_PID
trap 'kill "$mock_pid" 2>/dev/null || true' EXIT
read -r ready <&"${FILE_PORTAL_MOCK[0]}"
[[ "$ready" == ready ]]
"$java_executable" -Djava.awt.headless=true -Djava.library.path="$native_directory" \
    -cp "$test_classpath" org.vds1022.portal.FilePortalNativeTest
