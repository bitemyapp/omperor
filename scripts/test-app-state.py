#!/usr/bin/env python3
"""Check packaged channel identities and eval routing in an empty scratch HOME."""
import argparse
import json
import os
from pathlib import Path
import plistlib
import sqlite3
import subprocess
import tempfile

if not __debug__:
    raise SystemExit("Run this test harness without Python's -O option")


def invoke(binary, env, *args, expected=0):
    # ubs:ignore — this harness intentionally executes the user's named local build with an argv list, without a shell.
    result = subprocess.run([str(binary), *args], env=env, capture_output=True, text=True, timeout=30)
    assert result.returncode == expected, (args, result.returncode, result.stderr)
    # ubs:ignore — malformed CLI JSON is a test failure and should retain its traceback.
    return json.loads(result.stdout) if expected == 0 else result.stderr


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--production", required=True, type=Path)
    parser.add_argument("--development", required=True, type=Path)
    args = parser.parse_args()
    scratch = Path(tempfile.mkdtemp(prefix="omperor-state-routing-"))
    env = os.environ.copy()
    env.pop("OMPEROR_STATE_DIR", None)
    env.update(HOME=str(scratch), CFFIXED_USER_HOME=str(scratch))
    binaries = [args.production.resolve(), args.development.resolve()]
    paths = [invoke(binary, env, "--state-paths") for binary in binaries]
    for binary, resolved, channel, name, identifier in zip(
        binaries, paths, ["production", "development"], ["Omperor", "Omperor Dev"],
        ["com.bitemyapp.omperor", "com.bitemyapp.omperor.dev"],
    ):
        assert resolved["channel"] == channel, resolved
        assert Path(resolved["root"]) == scratch / "Library/Application Support" / name
        assert not Path(resolved["root"]).exists(), "state diagnostic created mutable state"
        plist_path = binary.parent.parent / "Info.plist"
        with plist_path.open("rb") as file:
            plist = plistlib.load(file)
        assert plist["CFBundleIdentifier"] == identifier, plist_path
        assert plist["CFBundleDisplayName"] == name, plist_path
        override = {**env, "OMPEROR_STATE_DIR": str(scratch / "campaign")}
        explicit = invoke(binary, override, "--state-paths")
        assert explicit["root"] == explicit["eval_root"] == str(scratch / "campaign")
        assert not (scratch / "campaign").exists()
        error = invoke(binary, {**env, "OMPEROR_STATE_DIR": "relative"}, "--state-paths", expected=2)
        assert "absolute" in error
    assert paths[0]["eval_root"] == paths[1]["root"] == paths[1]["eval_root"]
    for key in ("root", "preferences", "sessions", "data", "archives", "worktrees", "logs"):
        assert paths[0][key] != paths[1][key], key

    # A production executable's eval must initialize only Dev metadata.
    assert invoke(binaries[0], env, "eval", "accounts")["accounts"] == []  # ubs:ignore — fixture assertion; -O is rejected above.
    production_root = Path(paths[0]["root"])
    assert not production_root.exists()
    production_root.mkdir(parents=True)
    sentinel = Path(paths[0]["preferences"])
    sentinel.write_bytes(b"production sentinel: must never be opened by evals")
    with sqlite3.connect(paths[1]["preferences"]) as database:
        database.execute(
            "INSERT INTO accounts (account_id, provider, principal, label, authenticated_at_ms, is_default) "
            "VALUES (?, ?, ?, ?, ?, ?)",
            ("openai-codex:state-test", "openai-codex", None, "Dev fixture", 0, 1),
        )
    for binary in binaries:
        accounts = invoke(binary, env, "eval", "accounts")
        assert accounts["credentials_verified"] is False  # ubs:ignore — fixture assertion; -O is rejected above.
        assert [account["label"] for account in accounts["accounts"]] == ["Dev fixture"]
        isolated = invoke(binary, {**env, "OMPEROR_STATE_DIR": str(scratch / "campaign")}, "eval", "accounts")
        assert isolated["accounts"] == []  # ubs:ignore — fixture assertion; -O is rejected above.
    assert sentinel.read_bytes() == b"production sentinel: must never be opened by evals"
    assert list(production_root.iterdir()) == [sentinel]
    print(json.dumps({"result": "PASS", "artifacts": str(scratch), "paths": paths}, indent=2))


if __name__ == "__main__":
    main()
