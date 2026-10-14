#!/usr/bin/env python3
"""Stage the Linux build with compatibility patches; keep pinned vendors clean."""
import hashlib
from pathlib import Path
import shutil
import subprocess

root = Path(__file__).resolve().parent.parent
overlay = root / ".objc2-overlay"

def stage_vendor(name):
    source = root / "vendor" / name
    staged = overlay / name
    patches = sorted((root / "patches" / name).glob("*.patch"))
    revision = subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
    digest = hashlib.sha256(Path(__file__).read_bytes() + b"".join(p.read_bytes() for p in patches)).hexdigest()
    key = revision + ":" + digest
    stamp = staged / ".omperor-linux"
    if not stamp.exists() or stamp.read_text() != key:
        if staged.exists():
            shutil.rmtree(staged)
        subprocess.run(["git", "-c", "advice.detachedHead=false", "clone", "--quiet", "--shared", str(source), str(staged)], check=True)
        subprocess.run(["git", "-C", str(staged), "checkout", "--quiet", "--detach", revision], check=True)
        if name == "upleft":
            shutil.rmtree(staged / "vendor")
            (staged / "vendor").symlink_to(source / "vendor", target_is_directory=True)
            (staged / "corpus/generated").symlink_to(source / "corpus/generated", target_is_directory=True)
        for patch in patches:
            subprocess.run(["git", "-C", str(staged), "apply", str(patch)], check=True)
        stamp.write_text(key)
    return staged


source = root / "vendor/upleft"
if not (source / "corpus/generated/docs/Docs__STATUS.md").exists():
    subprocess.run(["python3", str(source / "scripts/build-corpus.py")], cwd=source, check=True)
upleft = stage_vendor("upleft")
sidestep = stage_vendor("sidestep")

build = overlay / "omperor"
build.mkdir(exist_ok=True)
# Keep include paths, corpus tests and helper discovery identical to the source.
for entry in root.iterdir():
    if entry.name.startswith(".") or entry.name in {"vendor", "Cargo.toml", "Cargo.lock", "Cargo.linux.lock", "target"}:
        continue
    link = build / entry.name
    if not link.exists():
        link.symlink_to(entry, target_is_directory=entry.is_dir())
(build / "Cargo.toml").write_text((root / "Cargo.toml").read_text())
vendor = build / "vendor"
vendor.mkdir(exist_ok=True)
for name, path in {"upleft": upleft, "sidestep": sidestep, "oh-my-pi": root / "vendor/oh-my-pi"}.items():
    link = vendor / name
    if link.is_symlink() and link.resolve() != path:
        link.unlink()
    if not link.exists():
        link.symlink_to(path, target_is_directory=True)
if (root / "Cargo.linux.lock").exists():
    shutil.copyfile(root / "Cargo.linux.lock", build / "Cargo.lock")
