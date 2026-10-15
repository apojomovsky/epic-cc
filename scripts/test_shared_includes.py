"""The vendored epic-docs copy must match its recorded checksum.

docs/snippets/ways-to-use-epic8.md is copied from apojomovsky/epic-docs;
docs/snippets/SHARED-INCLUDES records the hub commit and the sha256 of the
copy. These tests run check() against temporary repos, so no network is
touched: the fetch used by sync() is injected.
"""

import pathlib
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import shared_includes  # noqa: E402

REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
COMMIT = "2ad7d851c1d39888dda84adcbf9fa8d0667453c8"
HUB_TEXT = b"## Ways to use epic8\n\n- **Command line:** one line.\n"


def _write(root, rel, data):
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def _synced_root(root):
    shared_includes.sync(root, COMMIT, fetch=lambda commit: HUB_TEXT)
    return root


class TestCheckedInCopy(unittest.TestCase):
    def test_checked_in_copy_matches_record(self):
        self.assertEqual(shared_includes.check(REPO_ROOT), [])


class TestCheck(unittest.TestCase):
    def test_synced_copy_passes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = _synced_root(pathlib.Path(tmp))
            self.assertEqual(shared_includes.check(root), [])

    def test_local_edit_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = _synced_root(pathlib.Path(tmp))
            _write(root, shared_includes.COPY, HUB_TEXT + b"edited here\n")
            problems = shared_includes.check(root)
            self.assertEqual(len(problems), 1)
            self.assertIn("edited here", problems[0])

    def test_missing_copy_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = _synced_root(pathlib.Path(tmp))
            (root / shared_includes.COPY).unlink()
            problems = shared_includes.check(root)
            self.assertEqual(len(problems), 1)
            self.assertIn("missing", problems[0])

    def test_missing_record_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            _write(root, shared_includes.COPY, HUB_TEXT)
            problems = shared_includes.check(root)
            self.assertEqual(len(problems), 1)
            self.assertIn(shared_includes.RECORD, problems[0])

    def test_short_commit_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = _synced_root(pathlib.Path(tmp))
            _write(
                root,
                shared_includes.RECORD,
                shared_includes.render_record(
                    "2ad7d85", shared_includes.sha256_hex(HUB_TEXT)
                ).encode(),
            )
            problems = shared_includes.check(root)
            self.assertTrue(any("40-character" in p for p in problems))

    def test_wrong_source_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = _synced_root(pathlib.Path(tmp))
            text = (root / shared_includes.RECORD).read_text()
            _write(
                root,
                shared_includes.RECORD,
                text.replace("source = ", "source = other/", 1).encode(),
            )
            problems = shared_includes.check(root)
            self.assertTrue(any("source must be" in p for p in problems))

    def test_malformed_record_line_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = _synced_root(pathlib.Path(tmp))
            _write(root, shared_includes.RECORD, b"not a key value line\n")
            problems = shared_includes.check(root)
            self.assertEqual(len(problems), 1)
            self.assertIn("malformed", problems[0])


class TestSync(unittest.TestCase):
    def test_sync_writes_copy_and_record(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            digest = shared_includes.sync(root, COMMIT, fetch=lambda commit: HUB_TEXT)
            self.assertEqual(digest, shared_includes.sha256_hex(HUB_TEXT))
            self.assertEqual((root / shared_includes.COPY).read_bytes(), HUB_TEXT)
            fields = shared_includes.parse_record(
                (root / shared_includes.RECORD).read_text()
            )
            self.assertEqual(fields["commit"], COMMIT)
            self.assertEqual(fields["sha256"], digest)

    def test_sync_rewrites_an_edited_copy(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = _synced_root(pathlib.Path(tmp))
            _write(root, shared_includes.COPY, b"edited\n")
            shared_includes.sync(root, COMMIT, fetch=lambda commit: HUB_TEXT)
            self.assertEqual(shared_includes.check(root), [])

    def test_sync_rejects_a_branch_name_without_fetching(self):
        def no_fetch(commit):
            raise AssertionError("fetch must not run for a non-sha ref")

        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(ValueError):
                shared_includes.sync(pathlib.Path(tmp), "master", fetch=no_fetch)

    def test_sync_rejects_empty_hub_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(ValueError):
                shared_includes.sync(
                    pathlib.Path(tmp), COMMIT, fetch=lambda commit: b""
                )


if __name__ == "__main__":
    unittest.main()
