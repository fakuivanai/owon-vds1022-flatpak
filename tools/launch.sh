#!/usr/bin/env bash
set -euo pipefail

mkdir -p -- "$XDG_DATA_HOME"
exec /app/jre/bin/java -Duser.home="$XDG_DATA_HOME" \
    -Djava.library.path=/app/lib \
    -cp '/app/share/owon-vds1022/lib/*' com.owon.vds.tiny.Main "$@"
