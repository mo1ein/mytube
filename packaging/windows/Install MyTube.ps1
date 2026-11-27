$ErrorActionPreference = 'Stop'
$binary = Join-Path $PSScriptRoot 'mytube.exe'
if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
    throw 'Extract the installer archive before running Install MyTube.cmd.'
}
# Keep downloads for repairs and verify them before every extraction.
$cache = Join-Path $env:LOCALAPPDATA 'MyTube\Installer'
New-Item -ItemType Directory -Path $cache -Force | Out-Null
function Get-Sha256([string]$path) {
    $stream = [IO.File]::OpenRead($path)
    try {
        $algorithm = [Security.Cryptography.SHA256]::Create()
        try {
            return [BitConverter]::ToString($algorithm.ComputeHash($stream)).Replace('-', '')
        } finally {
            $algorithm.Dispose()
        }
    } finally {
        $stream.Dispose()
    }
}

function Get-VerifiedArchive([string]$url, [string]$sha256) {
    $archive = Join-Path $cache ([IO.Path]::GetFileName($url))
    if ((Test-Path -LiteralPath $archive -PathType Leaf) -and
        (Get-Sha256 $archive) -eq $sha256) {
        return $archive
    }
    $download = Join-Path $cache ([guid]::NewGuid().ToString())
    try {
        Write-Host "Downloading $([IO.Path]::GetFileName($url))..."
        & "$env:SystemRoot\System32\curl.exe" --fail --silent --show-error --location --retry 2 --output $download $url
        if ($LASTEXITCODE -ne 0) { throw "Download failed for $url" }
        if ((Get-Sha256 $download) -ne $sha256) {
            throw "Checksum verification failed for $url"
        }
        Move-Item -LiteralPath $download -Destination $archive -Force
    } finally {
        if (Test-Path -LiteralPath $download) { Remove-Item -LiteralPath $download }
    }
    return $archive
}

$ffmpegBuild = 'ffmpeg-9.1-essentials_build'
$ffmpeg = Get-VerifiedArchive "https://github.com/GyanD/codexffmpeg/releases/download/9.1/$ffmpegBuild.7z" '835B9A733445BA7A56A37A26CF7BDE84E2F3CE7DA2A8EC984276BB2ABB1B922C'
$mpv = Get-VerifiedArchive 'https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20261111/mpv-dev-x86_64-20261111-git-bcde6b93cc.7z' '273C079B11EA5575822F972756F4C9A71149A5C10130353ABEB8586738938452'
# This libmpv build imports vulkan-1.dll even when playback uses OpenGL.
$vulkanBuild = 'VulkanRT-X64-1.4.363.0-Components'
$vulkan = Get-VerifiedArchive "https://sdk.lunarg.com/sdk/download/1.4.363.0/windows/$vulkanBuild.zip" 'A25A927AA8B9F0371048F1861CF88AC3B9BC9B1FB332C42D897C8AB32695769A'
$destination = Join-Path $env:LOCALAPPDATA 'Programs\MyTube'
$stage = Join-Path $cache ([guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $stage | Out-Null
try {
    $tar = "$env:SystemRoot\System32\tar.exe"
    & $tar -xf $ffmpeg -C $stage "$ffmpegBuild/bin/ffmpeg.exe" "$ffmpegBuild/bin/ffprobe.exe" "$ffmpegBuild/LICENSE"
    if ($LASTEXITCODE -ne 0) { throw 'Could not extract FFmpeg and FFprobe.' }
    & $tar -xf $mpv -C $stage libmpv-2.dll
    if ($LASTEXITCODE -ne 0) { throw 'Could not extract the mpv playback engine.' }
    & $tar -xf $vulkan -C $stage "$vulkanBuild/x64/vulkan-1.dll" "$vulkanBuild/VulkanRT-License.txt"
    if ($LASTEXITCODE -ne 0) { throw 'Could not extract the Vulkan loader.' }
    $mediaTools = Join-Path $stage "$ffmpegBuild/bin"
    foreach ($tool in 'ffmpeg.exe', 'ffprobe.exe') {
        & (Join-Path $mediaTools $tool) -version | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "$tool could not run on this system." }
    }
    New-Item -ItemType Directory -Path $destination -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $mediaTools 'ffmpeg.exe'), (Join-Path $mediaTools 'ffprobe.exe'), (Join-Path $stage 'libmpv-2.dll') -Destination $destination -Force
    Copy-Item -LiteralPath (Join-Path $stage "$vulkanBuild/x64/vulkan-1.dll"), (Join-Path $stage "$vulkanBuild/VulkanRT-License.txt") -Destination $destination -Force
    Copy-Item -LiteralPath (Join-Path $stage "$ffmpegBuild/LICENSE") -Destination (Join-Path $destination 'FFmpeg-LICENSE.txt') -Force
} finally {
    Remove-Item -LiteralPath $stage -Recurse -Force
}
Copy-Item -LiteralPath $binary -Destination $destination -Force
Copy-Item -Path (Join-Path $PSScriptRoot '*-LICENSE.txt') -Destination $destination -Force
$menu = [Environment]::GetFolderPath([Environment+SpecialFolder]::Programs)
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut((Join-Path $menu 'MyTube.lnk'))
$shortcut.TargetPath = Join-Path $destination 'mytube.exe'
$shortcut.WorkingDirectory = $destination
$shortcut.Save()
