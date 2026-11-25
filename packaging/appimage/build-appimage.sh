#!/usr/bin/env bash
# Build a portable AppImage of Gateless Scholar.
#
# Portable by default: the binary is built against glibc 2.28 with cargo-zigbuild, so the
# AppImage runs on practically any Linux from 2018 on (Ubuntu 18.10+, Debian 10+, Fedora 29+).
# Graphics, Wayland and X11 libraries are loaded from the system at run time.
#
# Needs: cargo; cargo-zigbuild + zig (`cargo install cargo-zigbuild`, `pip install ziglang`),
# or set NATIVE=1 to build against this machine's glibc instead; appimagetool
# (https://github.com/AppImage/appimagetool) on PATH or in $APPIMAGETOOL.
set -euo pipefail
cd "$(dirname "$0")/../.."
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)

if [[ "${NATIVE:-0}" == "1" ]]; then
    cargo build --release --locked
    BIN=target/release/gateless
else
    cargo zigbuild --release --locked --target x86_64-unknown-linux-gnu.2.28
    BIN=target/x86_64-unknown-linux-gnu/release/gateless
fi

APPDIR=dist/Gateless.AppDir
rm -rf "$APPDIR"
install -Dm755 "$BIN" "$APPDIR/usr/bin/gateless"
install -Dm644 assets/org.gatelessscholar.Gateless.desktop "$APPDIR/usr/share/applications/org.gatelessscholar.Gateless.desktop"
install -Dm644 assets/org.gatelessscholar.Gateless.metainfo.xml "$APPDIR/usr/share/metainfo/org.gatelessscholar.Gateless.appdata.xml"
install -Dm644 assets/org.gatelessscholar.Gateless.svg "$APPDIR/usr/share/icons/hicolor/scalable/apps/org.gatelessscholar.Gateless.svg"
cp assets/org.gatelessscholar.Gateless.desktop "$APPDIR/"
# The top-level icon must be a PNG (appimagetool derives .DirIcon from it).
if command -v rsvg-convert >/dev/null; then
    rsvg-convert -w 256 -h 256 assets/org.gatelessscholar.Gateless.svg -o "$APPDIR/org.gatelessscholar.Gateless.png"
else
    magick -background none -density 384 assets/org.gatelessscholar.Gateless.svg -resize 256x256 "$APPDIR/org.gatelessscholar.Gateless.png"
fi
install -Dm644 "$APPDIR/org.gatelessscholar.Gateless.png" "$APPDIR/usr/share/icons/hicolor/256x256/apps/org.gatelessscholar.Gateless.png"
cat > "$APPDIR/AppRun" <<'RUN'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/bin/gateless" "$@"
RUN
chmod +x "$APPDIR/AppRun"

TOOL=${APPIMAGETOOL:-appimagetool}
OUT="dist/Gateless_Scholar-$VERSION-x86_64.AppImage"
# --appimage-extract-and-run lets appimagetool (itself an AppImage) run without FUSE.
ARCH=x86_64 "$TOOL" --appimage-extract-and-run "$APPDIR" "$OUT"
echo "Built $OUT"
