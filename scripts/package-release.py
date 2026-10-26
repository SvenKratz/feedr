#!/usr/bin/env python3
"""Build a versioned update archive from a native release executable."""
import hashlib
from pathlib import Path
import platform
import plistlib
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile

root = Path(__file__).resolve().parents[1]
version = tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"]
binary = root / "target/release/feedr"
assert subprocess.check_output([str(binary), "--version"], text=True).strip() == f"feedr {version}"
system = {"Darwin": "macos", "Linux": "linux"}[platform.system()]
arch = {"arm64": "aarch64", "aarch64": "aarch64", "x86_64": "x86_64"}[platform.machine()]
dist = root / "dist"
dist.mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(prefix="feedr-release-") as temporary:
    stage = Path(temporary)
    if system == "macos":
        app = stage / "Feedr.app/Contents"
        (app / "MacOS").mkdir(parents=True)
        (app / "Resources").mkdir()
        shutil.copy2(binary, app / "MacOS/feedr")
        shutil.copy2(root / "packaging/Feedr.icns", app / "Resources/Feedr.icns")
        info = plistlib.loads((root / "packaging/Info.plist").read_bytes())
        info.update(CFBundleShortVersionString=version, CFBundleVersion=version)
        (app / "Info.plist").write_bytes(plistlib.dumps(info))
        subprocess.run(["codesign", "--force", "--sign", "-", str(app.parent)], check=True)
        subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app.parent)], check=True)
        archive = dist / f"feedr-v{version}-{system}-{arch}.zip"
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as output:
            for path in sorted(stage.rglob("*")):
                if path.is_file():
                    output.write(path, path.relative_to(stage))
    else:
        for source, name in [(binary, "feedr"), (root / "README.md", "README.md"), (root / "LICENSE", "LICENSE"), (root / "packaging/feedr.desktop", "feedr.desktop"), (root / "packaging/feedr.svg", "feedr.svg")]:
            shutil.copy2(source, stage / name)
        archive = dist / f"feedr-v{version}-{system}-{arch}.tar.gz"
        with tarfile.open(archive, "w:gz") as output:
            for path in sorted(stage.iterdir()):
                output.add(path, arcname=path.name)
checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
subprocess.run([str(binary), "update-verify", "--archive", str(archive)], check=True)
archive.with_name(archive.name + ".sha256").write_text(f"{checksum}  {archive.name}\n")
print(archive)
