#!/bin/sh
# Build FerrisCad as an AppImage: app/dist/FerrisCad-<arch>.AppImage
# Usage (from anywhere):  sh app/packaging/build-appimage.sh
set -eu
APP="$(cd "$(dirname "$0")/.." && pwd)"
PKG="$APP/packaging"
OUT="$APP/dist"
TOOLS="$PKG/tools"
ARCH="$(uname -m)"

echo "== 1/3 Release build"
cd "$APP"
cargo build --release

echo "== 2/3 AppDir"
mkdir -p "$APP/target/appimage"
WORK="$(mktemp -d "$APP/target/appimage/build.XXXXXX")"
DIR="$WORK/AppDir"
mkdir -p "$DIR/usr/bin" "$DIR/usr/share/applications" "$DIR/usr/share/icons/hicolor/scalable/apps"
cp "$APP/target/release/ferriscad" "$DIR/usr/bin/"
strip "$DIR/usr/bin/ferriscad" 2>/dev/null || true
cp "$PKG/AppRun" "$DIR/AppRun"
chmod +x "$DIR/AppRun"
cp "$PKG/ferriscad.desktop" "$DIR/"
cp "$PKG/ferriscad.desktop" "$DIR/usr/share/applications/"
cp "$PKG/ferriscad.svg" "$DIR/"
cp "$PKG/ferriscad.svg" "$DIR/usr/share/icons/hicolor/scalable/apps/"
ln -s ferriscad.svg "$DIR/.DirIcon"

echo "== 3/3 AppImage"
TOOL="$TOOLS/appimagetool-$ARCH.AppImage"
if [ ! -x "$TOOL" ]; then
  mkdir -p "$TOOLS"
  echo "Downloading appimagetool (one time)..."
  curl -fL --retry 3 -o "$TOOL.part" "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$ARCH.AppImage"
  mv "$TOOL.part" "$TOOL"
  chmod +x "$TOOL"
fi
mkdir -p "$OUT"
# APPIMAGE_EXTRACT_AND_RUN lets the tool run without FUSE.
APPIMAGE_EXTRACT_AND_RUN=1 ARCH="$ARCH" "$TOOL" --no-appstream "$DIR" "$OUT/FerrisCad-$ARCH.AppImage"
echo "Done: $OUT/FerrisCad-$ARCH.AppImage"
