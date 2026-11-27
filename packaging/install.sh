: <<'MYTUBE_WINDOWS'
@echo off
setlocal
set "MYTUBE_VERSION=1.0.0"
if not defined MYTUBE_RELEASE set "MYTUBE_RELEASE=https://github.com/mo1ein/mytube/releases/download/v%MYTUBE_VERSION%"
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -Command ^
  "$ErrorActionPreference = 'Stop';" ^
  "if ('AMD64' -notin @($env:PROCESSOR_ARCHITECTURE, $env:PROCESSOR_ARCHITEW6432)) { throw 'MyTube requires Windows x64.' };" ^
  "$work = New-Item -ItemType Directory -Path (Join-Path ([IO.Path]::GetTempPath()) ([guid]::NewGuid().ToString()));" ^
  "try {" ^
  "  $asset = 'mytube-' + $env:MYTUBE_VERSION + '-windows-x64.zip';" ^
  "  $zip = Join-Path $work.FullName $asset;" ^
  "  Write-Host ('Downloading ' + $asset);" ^
  "  curl.exe --fail --location --show-error --silent --retry 2 ($env:MYTUBE_RELEASE + '/' + $asset) -o $zip;" ^
  "  if ($LASTEXITCODE -ne 0) { throw 'Could not download the MyTube release.' };" ^
  "  & (Join-Path $env:SystemRoot 'System32\tar.exe') -xf $zip -C $work.FullName;" ^
  "  if ($LASTEXITCODE -ne 0) { throw 'Could not extract the MyTube release.' };" ^
  "  & (Join-Path $work.FullName 'Install MyTube.ps1');" ^
  "  Write-Host 'Installed MyTube. Open it from the Start menu.';" ^
  "} finally { Remove-Item -LiteralPath $work.FullName -Recurse -Force }"
exit /b %errorlevel%
MYTUBE_WINDOWS
# The block above runs when Windows opens this file as a .cmd; sh skips it.
set -eu

version=$(sed -n 's/^set "MYTUBE_VERSION=\([^"]*\)".*/\1/p' "$0")
release=${MYTUBE_RELEASE:-"https://github.com/mo1ein/mytube/releases/download/v$version"}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
trap 'exit 1' HUP INT TERM
case "$(uname -s)" in
    Darwin)
        case "$(uname -m)" in
            arm64) arch=arm64 ;;
            x86_64)
                arch=x64
                if [ "$(sysctl -in sysctl.proc_translated 2>/dev/null)" = 1 ]; then
                    arch=arm64
                fi
                ;;
            *) echo "MyTube requires an Apple silicon or Intel Mac." >&2; exit 1 ;;
        esac
        asset="mytube-$version-macos-$arch.zip"
        ;;
    Linux)
        if ! command -v apt-get >/dev/null || ! command -v dpkg >/dev/null ||
           [ "$(dpkg --print-architecture)" != amd64 ]; then
            echo "The Linux release requires x86-64 Debian or Ubuntu with APT." >&2
            exit 1
        fi
        asset="mytube_${version}_amd64.deb"
        ;;
    MINGW*|MSYS*|CYGWIN*)
        cp "$0" "$work/install-mytube.cmd"
        MYTUBE_CMD=$(cygpath -aw "$work/install-mytube.cmd")
        export MYTUBE_CMD
        powershell.exe -NoLogo -NoProfile -Command '& $env:MYTUBE_CMD; exit $LASTEXITCODE'
        exit
        ;;
    *) echo "MyTube supports macOS, Windows x64, and x86-64 Debian/Ubuntu." >&2; exit 1 ;;
esac

printf 'Downloading %s\n' "$asset"
curl --fail --location --show-error --silent --retry 2 "$release/$asset" -o "$work/$asset"
case "$asset" in
    *.zip)
        /usr/bin/ditto -x -k "$work/$asset" "$work"
        /bin/bash "$work/Install MyTube.command"
        ;;
    *.deb)
        if [ "$(id -u)" -eq 0 ]; then
            apt-get install "$work/$asset"
        else
            sudo apt-get install "$work/$asset"
        fi
        ;;
esac
