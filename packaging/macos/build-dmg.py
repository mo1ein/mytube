"""Build a drag-installable disk image from the signed MyTube app bundle."""

import os
from pathlib import Path
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory


def output(*args):
    return subprocess.check_output(args, text=True).strip()


def run(*args):
    subprocess.run(args, check=True)


def dependencies(binary):
    lines = output("otool", "-L", binary).splitlines()[1:]
    if binary.suffix == ".dylib":
        lines = lines[1:]  # The first entry is the library's own install name.
    return [line.strip().split(" (compatibility version")[0] for line in lines]


def resolve_dependency(name, binary):
    if name.startswith("@loader_path/"):
        return binary.parent / name.removeprefix("@loader_path/")
    if name.startswith("@rpath/"):
        commands = output("otool", "-l", binary).split("cmd LC_RPATH")[1:]
        for command in commands:
            directory = command.split("path ", 1)[1].split(" (offset", 1)[0]
            directory = directory.replace("@loader_path", str(binary.parent))
            candidate = Path(directory) / name.removeprefix("@rpath/")
            if candidate.is_absolute() and candidate.is_file():
                return candidate
    if not Path(name).is_absolute():
        raise ValueError(f"Could not resolve {name} in {binary}")
    return Path(name)


def bundle_runtime(app):
    contents = app / "Contents"
    executables = contents / "MacOS"
    frameworks = contents / "Frameworks"
    frameworks.mkdir()
    ffmpeg = Path(output("brew", "--prefix", "ffmpeg"))
    mpv = Path(output("brew", "--prefix", "mpv"))
    pending = [
        (ffmpeg / "bin" / name, executables / name) for name in ("ffmpeg", "ffprobe")
    ]
    # MyTube looks for libmpv beside its own executable.
    libmpv = "libmpv.2.dylib"
    pending.append((mpv / "lib" / libmpv, executables / libmpv))
    copied = {}
    packages = set()
    while pending:
        source, target = pending.pop()
        source = source.resolve(strict=True)
        if target in copied:
            if copied[target] != source:
                raise ValueError(f"Two different libraries would be packaged at {target}")
            continue
        copied[target] = source
        packages.update(p for p in source.parents if (p / "INSTALL_RECEIPT.json").is_file())
        shutil.copyfile(source, target)
        target.chmod(0o755)
        changes = []
        if source.suffix == ".dylib":
            changes.extend(["-id", f"@loader_path/{target.name}"])
        for name in dependencies(source):
            if name.startswith(("/usr/lib/", "/System/Library/")):
                continue
            library = resolve_dependency(name, source).resolve(strict=True)
            dependency = frameworks / library.name
            pending.append((library, dependency))
            relative = os.path.relpath(dependency, target.parent)
            changes.extend(["-change", name, f"@loader_path/{relative}"])
        if changes:
            run("install_name_tool", *changes, target)
        run("codesign", "--force", "--sign", "-", "--timestamp=none", target)

    for package in packages:
        notices = contents / "Resources" / "Licenses" / package.parent.name
        notices.mkdir(parents=True, exist_ok=True)
        for file in package.iterdir():
            if file.is_file() and file.name.lower().startswith(("license", "copying", "copyright", "notice", "sbom.spdx")):
                shutil.copyfile(file, notices / file.name)


def build(app, dmg):
    with TemporaryDirectory(dir=dmg.parent) as work:
        stage = Path(work) / "volume"
        stage.mkdir()
        bundled = stage / "MyTube.app"
        run("ditto", app, bundled)
        bundle_runtime(bundled)
        run("codesign", "--force", "--sign", "-", "--timestamp=none", bundled)
        run("codesign", "--verify", "--deep", "--strict", bundled)
        (stage / "Applications").symlink_to("/Applications")
        image = Path(work) / "MyTube.dmg"
        run("hdiutil", "create", "-volname", "MyTube", "-format", "UDZO",
            "-srcfolder", stage, image)
        image.replace(dmg)


if __name__ == "__main__":
    app_path, image_path = map(Path, sys.argv[1:])
    build(app_path, image_path)
