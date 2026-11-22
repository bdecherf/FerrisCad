#!/bin/sh
# Inside a Flatpak sandbox there is no C compiler/linker: use the host's (same home paths).
if [ -f /.flatpak-info ] && command -v flatpak-spawn >/dev/null 2>&1; then
  exec flatpak-spawn --host --env=TMPDIR="$TMPDIR" cc "$@"
fi
exec "${HOST_CC:-cc}" "$@"
