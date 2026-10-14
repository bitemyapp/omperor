#!/usr/bin/env python3
"""Exercise real RPC helper isolation using only the vendored loopback provider.

Requires Python 3.10+ (the vendored vendor/python/install/bin/python3 works).
The default serializes first-time store initialization; --cold-start races it.
--isolated-stores checks separate app data roots in the same physical project;
--other-helper additionally exercises two different packaged executables.
Artifacts are retained in the printed temporary directory. No user credentials
or project files are used. Run after scripts/build-app.sh, or pass --helper.
"""
import argparse
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
if not __debug__:
    raise SystemExit("Run this test harness without Python's -O option")
sys.path.insert(0, str(ROOT / "vendor/oh-my-pi/scripts/qa"))
if sys.version_info < (3, 10):
    raise SystemExit("Python 3.10+ required; use vendor/oh-my-pi/vendor/python/install/bin/python3")
from harness import CONFIG_TOML, MODELS_TOML, MockModel, call


class Helper:
    def __init__(self, executable, root, name, extra=(), storage=None):
        self.name = name
        self.events = []
        self.queue = queue.Queue()
        self.log = (root / f"{name}.jsonl").open("w")
        self.stderr = (root / f"{name}.stderr").open("w")
        env = {key: os.environ[key] for key in ("PATH", "TMPDIR") if key in os.environ}
        storage = storage or root
        data = storage / "data"
        env.update(HOME=str(root / "home"), OMP_DATA_DIR=str(data),
                   OMP_STATE_DIR=str(data / "state"), OMP_CACHE_DIR=str(data / "cache"),
                   OMP_CONFIG_DIR=str(data / "config"), OMP_WORKTREE_DIR=str(data / "worktrees"),
                   OMP_PROFILE="default", OMP_LLM_KEY_SOURCE="local-file")
        self.process = subprocess.Popen(
            [str(executable), "rpc", "--model", "mock", "--no-ext", "--no-title",
             "--no-lsp", "--no-skills", "--no-context-files", "--no-rules",
             "--auto-approve", "--session-dir", str(storage / "sessions"), *extra],
            cwd=root / "project", env=env, stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=self.stderr, start_new_session=True)
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()

    def read(self):
        stream = self.process.stdout
        try:
            while line := stream.readline():
                if line.lower().startswith(b"content-length:"):
                    length = int(line.split(b":", 1)[1])
                    while stream.readline().strip():
                        pass
                    line = stream.read(length)
                if not line.strip():
                    continue
                event = json.loads(line)
                record = dict(at=time.monotonic(), pid=self.process.pid, event=event)
                self.log.write(json.dumps(record) + "\n")
                self.log.flush()
                self.events.append(record)
                self.queue.put(event)
        finally:
            self.queue.put(None)

    def send(self, kind, **params):
        body = json.dumps(dict(id=f"{self.name}-{kind}", type=kind, **params)).encode()
        self.process.stdin.write(body + b"\n")
        self.process.stdin.flush()

    def wait(self, kind, timeout=60):
        deadline = time.monotonic() + timeout
        while True:
            event = self.queue.get(timeout=max(0.01, deadline - time.monotonic()))
            if event is None:
                raise AssertionError(f"{self.name} exited waiting for {kind}; inspect stderr")
            if event.get("type") == kind:
                return event
            if time.monotonic() >= deadline:
                raise TimeoutError(f"{self.name}: {kind}")

    def identity(self):
        self.wait("ready")
        self.send("get_state")
        response = self.wait("response")
        assert response["success"], response
        return response["data"]

    def stop(self):
        self.send("shutdown")
        self.wait("session_shutdown", 30)
        self.process.stdin.close()
        assert self.process.wait(timeout=30) == 0

    def cleanup(self):
        if self.process.poll() is None:
            self.process.kill()
        self.process.wait(timeout=10)
        self.process.stdin.close()
        self.reader.join(timeout=10)
        assert not self.reader.is_alive(), "RPC reader did not settle"
        self.process.stdout.close()
        self.stderr.close()
        self.log.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cold-start", action="store_true", help="Also race first-time credential store initialization")
    parser.add_argument("--helper", type=Path, default=ROOT / "target/Omperor Dev.app/Contents/Helpers/omp")
    parser.add_argument("--other-helper", type=Path, help="Use a second bundle's helper for isolated-store checks")
    parser.add_argument("--isolated-stores", action="store_true", help="Separate both stores while sharing HOME and the project")
    args = parser.parse_args()
    if args.other_helper and not args.isolated_stores:
        parser.error("--other-helper requires --isolated-stores")
    args.helper = args.helper.resolve(strict=True)
    if args.other_helper:
        args.other_helper = args.other_helper.resolve(strict=True)
    root = Path(tempfile.mkdtemp(prefix="omperor-concurrent-rpc-"))
    print(f"artifacts: {root}", flush=True)
    for directory in ("home", "data", "project", "sessions"):
        (root / directory).mkdir()
    helpers = []
    stores = [root / "production", root / "development"] if args.isolated_stores else [root, root]
    for store in stores:
        for directory in ("data", "sessions"):
            (store / directory).mkdir(parents=True, exist_ok=True)
    # The existing QA fixture owns wire encoding. Hold both responses until
    # both real helper requests have arrived, proving actual inference overlap.
    with MockModel("concurrent-safe", loop=True) as mock:
        original = mock._sse_events
        barrier = threading.Barrier(2)
        arrivals = []
        def held_events(reply, ordinal, body):
            arrivals.append(time.monotonic())
            barrier.wait(timeout=45)
            return original(reply, ordinal, body)
        mock._sse_events = held_events
        for store in stores:
            (store / "data/models.toml").write_text(MODELS_TOML.format(port=mock.port))
            (store / "data/config.toml").write_text(CONFIG_TOML)
        try:
            a = Helper(args.helper, root, "a", storage=stores[0])
            helpers.append(a)
            first_start = None if args.cold_start else a.identity()
            b = Helper(args.other_helper or args.helper, root, "b", storage=stores[1])
            helpers.append(b)
            starts = [first_start or a.identity(), b.identity()]
            paths = [Path(start["sessionFile"]) for start in starts]
            assert paths[0] != paths[1], starts
            for path, store in zip(paths, stores):
                assert path.is_relative_to(store / "sessions"), (path, store)  # ubs:ignore — fixture assertion; -O is rejected above.
            for h in (a, b):
                h.send("prompt", message=f"isolated-{h.name}")
            for h in (a, b):
                h.wait("agent_end")
            assert len(arrivals) == 2, arrivals
            for index, path in enumerate(paths):
                text = path.read_text()
                own, other = ("a", "b") if index == 0 else ("b", "a")
                assert f"isolated-{own}" in text and f"isolated-{other}" not in text
                assert "concurrent-safe" in text
            contender = Helper(args.helper, root, "contender", ("--resume", str(paths[0])), storage=stores[0])
            helpers.append(contender)
            assert contender.process.wait(timeout=30) != 0, "second writer admitted"
            rejection = (root / "contender.stderr").read_text()
            assert "lock" in rejection.lower() or "writer" in rejection.lower(), rejection
            # Kill only our idle helper; another real session must continue and
            # explicit resume must not replay the already accepted first prompt.
            a.process.kill()
            assert a.process.wait(timeout=30) != 0
            mock._sse_events = original
            b.send("prompt", message="survivor-b")
            b.wait("agent_end")
            # Start an actual tool descendant, then shut down during that tool.
            mock.replies = (call("bash", command="/usr/bin/python3 -c 'import os,time; open(\"descendant.pid\",\"w\").write(str(os.getpid())); time.sleep(120)'", timeout=130, i="Testing shutdown"),)
            b.send("prompt", message="start-owned-descendant")
            marker = root / "project/descendant.pid"
            deadline = time.monotonic() + 30
            while not marker.exists() and time.monotonic() < deadline:
                time.sleep(0.05)
            assert marker.exists(), "bash descendant did not start"
            descendant = int(marker.read_text().strip())
            b.stop()
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                try:
                    os.kill(descendant, 0)
                except ProcessLookupError:
                    break
                time.sleep(0.05)
            else:
                raise AssertionError(f"owned descendant survived shutdown: {descendant}")
            resumed = Helper(args.helper, root, "resumed", ("--resume", str(paths[0])), storage=stores[0])
            helpers.append(resumed)
            assert Path(resumed.identity()["sessionFile"]) == paths[0]
            assert paths[0].read_text().count('isolated-a') == 1, "resume replayed an accepted prompt"
            resumed.stop()
            forked = Helper(args.helper, root, "forked", ("--fork", str(paths[0])), storage=stores[0])
            helpers.append(forked)
            fork_path = Path(forked.identity()["sessionFile"])
            assert fork_path not in paths
            assert "isolated-a" in fork_path.read_text()
            forked.stop()
            report = dict(result="PASS", helper=str(args.helper), other_helper=str(args.other_helper or args.helper),
                          isolated_stores=args.isolated_stores, data_roots=[str(store / "data") for store in stores],
                          helpers=[dict(name=h.name, pid=h.process.pid, exit=h.process.returncode) for h in helpers],
                          sessions=starts, overlapping_request_arrivals=arrivals,
                          writer_rejection=rejection, killed_helper=a.process.pid, survivor_completed="survivor-b", stopped_descendant=descendant, fork_journal=str(fork_path),
                          bootstrap="concurrent-cold" if args.cold_start else "serialized")
            (root / "report.json").write_text(json.dumps(report, indent=2))
            print(json.dumps(report, indent=2), flush=True)
        finally:
            for helper in helpers:
                helper.cleanup()


if __name__ == "__main__":
    main()
