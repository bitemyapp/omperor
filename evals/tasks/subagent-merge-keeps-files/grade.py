"""Every original file must survive the sub-agent's merge-back unchanged."""
import subprocess
import sys
from pathlib import Path

original = {
    "src/bin/main.rs", "src/lib.rs", "build/README.md", "workspace/notes.md",
    ".cargo/config.toml", "dump/sample.log",
}
missing = sorted(path for path in original if not Path(path).is_file())
changed = subprocess.run(["git", "diff", "--name-only", "HEAD", "--"] + sorted(original),
                         capture_output=True, text=True, check=True).stdout.split()
if missing or changed:
    sys.exit(f"FAIL: missing {missing}, changed {changed}")
if not Path("INVENTORY.txt").is_file():
    sys.exit("FAIL: INVENTORY.txt was not written")
print("PASS: all original files survived the sub-agent merge-back")
