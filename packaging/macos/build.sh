#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
binary="${1:-target/release/mytube}"
output="${2:-target/install/macos}"
bundle="$output/MyTube.app/Contents"
test -x "$binary"
version="$(sed -n 's/^version = "\([^"]*\)"$/\1/p' Cargo.toml)"
mkdir -p "$bundle/MacOS" "$bundle/Resources"
install -m 755 "$binary" "$bundle/MacOS/mytube"
install -m 644 assets/*-LICENSE.txt "$bundle/Resources/"
install -m 755 'packaging/macos/Install MyTube.command' "$output/Install MyTube.command"
cat > "$bundle/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>mytube</string>
  <key>CFBundleIdentifier</key><string>app.mytube.desktop</string>
  <key>CFBundleName</key><string>MyTube</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
codesign --force --sign - --timestamp=none "$output/MyTube.app"
