"""Keep source pins strict while allowing unrelated code commits after freeze."""

from __future__ import annotations

import hashlib
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import validate_candidate as validator


class SourceFenceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.git("init", "-q")
        self.git("config", "user.name", "Offline Fixture")
        self.git("config", "user.email", "fixture@example.invalid")
        self.path = "docs/adr/0001-fixture.md"
        self.content = b"# Synthetic source\nKeep the rollback condition.\n"
        target = self.root / self.path
        target.parent.mkdir(parents=True)
        target.write_bytes(self.content)
        self.commit()
        self.source_commit = self.git("rev-parse", "HEAD")
        self.source = {
            "path": self.path,
            "git_blob_oid": self.git("rev-parse", f"HEAD:{self.path}"),
            "content_sha256": hashlib.sha256(self.content).hexdigest(),
            "line_count": 2,
        }
        self.root_patch = patch.object(validator, "ROOT", self.root)
        self.root_patch.start()
        self.addCleanup(self.root_patch.stop)

    def git(self, *args: str) -> str:
        return subprocess.check_output(
            ["git", *args], cwd=self.root, text=True, stderr=subprocess.DEVNULL
        ).strip()

    def commit(self) -> None:
        self.git("add", ".")
        self.git("-c", "commit.gpgsign=false", "commit", "-qm", "synthetic fixture")

    def verify(self) -> None:
        validator.verify_source_fences(self.source, self.source_commit)

    def test_unrelated_head_advance_preserves_frozen_sources(self) -> None:
        (self.root / "code.rs").write_text("fn fixture() {}\n", encoding="utf-8")
        self.commit()
        self.assertNotEqual(self.git("rev-parse", "HEAD"), self.source_commit)
        self.verify()

    def test_worktree_source_drift_is_rejected(self) -> None:
        (self.root / self.path).write_bytes(self.content + b"Changed condition.\n")
        with self.assertRaises(AssertionError):
            self.verify()

    def test_committed_drift_cannot_be_hidden_by_restoring_worktree(self) -> None:
        (self.root / self.path).write_bytes(self.content + b"Changed condition.\n")
        self.commit()
        (self.root / self.path).write_bytes(self.content)
        with self.assertRaises(AssertionError):
            self.verify()

    def test_original_blob_hash_and_line_count_pins_remain_required(self) -> None:
        for field, bad in [("git_blob_oid", "0" * 40), ("content_sha256", "0" * 64), ("line_count", 3)]:
            with self.subTest(field=field), patch.dict(self.source, {field: bad}):
                with self.assertRaises(AssertionError):
                    self.verify()


if __name__ == "__main__":
    unittest.main()
