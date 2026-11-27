#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
binary="${1:-target/release/mytube}"
output="${2:-target/install/linux}"
test -x "$binary"

version="$(sed -n 's/^version = "\([^"]*\)"$/\1/p' Cargo.toml)"
arch="$(dpkg --print-architecture)"
# Use the newest glibc symbol imported by this build, including on non-Ubuntu builders.
glibc="$(objdump -T "$binary" | sed -n 's/.*(GLIBC_\([0-9.]*\)).*/\1/p' | sort -Vu | tail -n 1)"
test -n "$glibc"
mkdir -p "$output"
stage="$(mktemp -d "$output/deb-stage.XXXXXX")"
chmod 755 "$stage"
trap 'rm -r "$stage"' EXIT
install -D -m 755 "$binary" "$stage/usr/bin/mytube"
install -D -m 644 packaging/linux/mytube.desktop "$stage/usr/share/applications/mytube.desktop"
install -D -m 644 assets/mytube.png "$stage/usr/share/icons/hicolor/256x256/apps/mytube.png"
install -D -m 644 assets/Inter-LICENSE.txt "$stage/usr/share/doc/mytube/copyright"
for license in assets/*-LICENSE.txt; do
    install -m 644 "$license" "$stage/usr/share/doc/mytube/"
done
mkdir -p "$stage/DEBIAN"
cat > "$stage/DEBIAN/control" <<CONTROL
Package: mytube
Version: $version
Section: video
Priority: optional
Architecture: $arch
Maintainer: MyTube contributors
Depends: libc6 (>= $glibc), libgcc-s1, libgl1, libegl1, libx11-6, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, mpv, libmpv2, ffmpeg, curl, ca-certificates, xdg-utils, zenity
Recommends: xdg-desktop-portal
Description: Desktop library for local videos
 Index, browse, organize, and play video files stored on local drives.
CONTROL
dpkg-deb --root-owner-group --build "$stage" "$output/mytube_${version}_${arch}.deb"
