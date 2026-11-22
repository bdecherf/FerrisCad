#!/bin/sh
if [ -f /.flatpak-info ] && command -v flatpak-spawn >/dev/null 2>&1; then
  exec flatpak-spawn --host ar "$@"
fi
exec ar "$@"
