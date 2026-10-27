"""Tests for publishing immutable screenshot gallery snapshots."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("publish-screenshots.sh").resolve()


class PublishScreenshotsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.remote = self.root / "remote.git"
        subprocess.run(["git", "init", "--bare", "-q", str(self.remote)], check=True)

    def environment(self, label):
        images = self.root / label
        images.mkdir()
        (images / "perseid-main.png").write_bytes(label.encode())
        env = os.environ.copy()
        env.update({
            "SCREENSHOTS_REMOTE": str(self.remote),
            "SCREENSHOTS_DIR": str(images),
            "GITHUB_SHA": label,
            "GITHUB_SERVER_URL": "https://github.com",
            "GITHUB_REPOSITORY": "example/perseid",
            "GITHUB_RUN_ID": "123",
            "GITHUB_RUN_ATTEMPT": "1",
        })
        return env

    def show(self, sha, path):
        return subprocess.check_output(["git", "--git-dir", str(self.remote), "show", f"{sha}:{path}"])

    def test_sequential_publishes_keep_each_snapshot_reachable(self):
        first = subprocess.check_output([str(SCRIPT)], env=self.environment("first"), text=True).strip()
        second = subprocess.check_output([str(SCRIPT)], env=self.environment("second"), text=True).strip()
        self.assertNotEqual(first, second)
        self.assertEqual(self.show(first, "perseid-main.png"), b"first")
        self.assertEqual(self.show(second, "perseid-main.png"), b"second")
        first_index = self.show(first, "index.md").decode()
        self.assertIn("Generated from `first`", first_index)
        self.assertIn("![Proxy - HTTP history](perseid-main.png)", first_index)
        self.assertNotIn("screenshots-preview/", first_index)
        subprocess.run(["git", "--git-dir", str(self.remote), "merge-base", "--is-ancestor", first, second], check=True)

    def test_concurrent_publishes_retry_without_losing_either_commit(self):
        # Hold each run at its first push so both initially publish the same
        # parent. One push must be rejected and retried on the winner's commit.
        barrier = self.root / "barrier"
        barrier.mkdir()
        wrapper_dir = self.root / "bin"
        wrapper_dir.mkdir()
        wrapper = wrapper_dir / "git"
        wrapper.write_text('''#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == push && ! -e "$BARRIER_DIR/$TEST_RUN.done" ]]; then
  touch "$BARRIER_DIR/$TEST_RUN.done"
  for i in {1..200}; do
    [[ -e "$BARRIER_DIR/one.done" && -e "$BARRIER_DIR/two.done" ]] && break
    sleep 0.05
  done
  [[ -e "$BARRIER_DIR/one.done" && -e "$BARRIER_DIR/two.done" ]]
fi
exec "$REAL_GIT" "$@"
''')
        wrapper.chmod(0o755)
        runs = []
        for name in ("one", "two"):
            env = self.environment(name)
            env.update({"PATH": str(wrapper_dir) + os.pathsep + env["PATH"],
                        "REAL_GIT": shutil.which("git"), "BARRIER_DIR": str(barrier), "TEST_RUN": name})
            runs.append(subprocess.Popen([str(SCRIPT)], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True))
        shas = []
        for run in runs:
            out, err = run.communicate(timeout=30)
            self.assertEqual(run.returncode, 0, err)
            shas.append(out.strip())
        for sha, name in zip(shas, ("one", "two")):
            self.assertEqual(self.show(sha, "perseid-main.png"), name.encode())
            self.assertIn(f"Generated from `{name}`", self.show(sha, "index.md").decode())
            subprocess.run(["git", "--git-dir", str(self.remote), "merge-base", "--is-ancestor", sha, "refs/heads/screenshots-preview"], check=True)


if __name__ == "__main__":
    unittest.main()
