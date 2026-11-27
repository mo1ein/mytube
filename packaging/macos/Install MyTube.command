#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
if [[ "$(uname -s)" != Darwin || ! -x MyTube.app/Contents/MacOS/mytube ]]; then
    printf '%s\n' 'Run this installer beside MyTube.app on macOS.' >&2
    exit 1
fi

find_brew() {
    for brew in /opt/homebrew/bin/brew /usr/local/bin/brew; do
        if [[ -x "$brew" ]]; then printf '%s\n' "$brew"; return; fi
    done
    command -v brew
}

if ! brew="$(find_brew)"; then
    printf '%s\n' 'Installing Homebrew to set up the media tools. macOS may ask for your password.'
    bootstrap="$(/usr/bin/curl --fail --silent --show-error --location https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
    /bin/bash -c "$bootstrap"
    brew="$(find_brew)"
fi
"$brew" install mpv ffmpeg
brew_bin="$("$brew" --prefix ffmpeg)/bin"
for tool in ffmpeg ffprobe; do
    "$brew_bin/$tool" -version >/dev/null
done
libmpv="$("$brew" --prefix mpv)/lib/libmpv.2.dylib"
if [[ ! -f "$libmpv" ]]; then
    printf 'Homebrew did not install libmpv at %s\n' "$libmpv" >&2
    exit 1
fi

destination="$HOME/Applications/MyTube.app"
if [[ -e "$destination" ]]; then
    bundle_id="$(/usr/libexec/PlistBuddy -c 'Print CFBundleIdentifier' "$destination/Contents/Info.plist" 2>/dev/null || true)"
    if [[ "$bundle_id" != app.mytube.desktop ]]; then
        printf 'A different app already exists at %s\n' "$destination" >&2
        exit 1
    fi
fi
mkdir -p "$HOME/Applications"
/usr/bin/ditto MyTube.app "$destination"
# Finder launches have a limited PATH. This also supports custom Homebrew prefixes.
printf '%s\n' "$brew_bin" > "$destination/Contents/Resources/media-bin"
printf '%s\n' "$libmpv" > "$destination/Contents/Resources/libmpv-path"
/usr/bin/codesign --force --sign - --timestamp=none "$destination"
printf 'Installed %s\n' "$destination"
