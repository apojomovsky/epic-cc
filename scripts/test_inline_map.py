"""Coverage for the inline-map generator's define diff and attribution.

Fixtures are hand-written IR fragments, so a failure names the folding
shape that broke rather than a shifted listing total. Run by
scripts/ci-test.sh alongside the cargo suites.
"""

import importlib.util
import pathlib
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "inline-map.py"

_spec = importlib.util.spec_from_file_location("inline_map", SCRIPT)
im = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(im)


def module(*bodies):
    return "\n".join(bodies) + "\n"


LEAF = """define void @leaf() {
  ret void
}"""


CALLER = """define void @caller() {
  call void @leaf()
  ret void
}"""


class InlineMapTest(unittest.TestCase):
    def test_single_caller_fold_maps_to_caller(self):
        clusters, skipped = im.inline_map(module(LEAF, CALLER), module(CALLER))
        self.assertEqual(clusters, {"caller": ["leaf"]})
        self.assertEqual(skipped, [])

    def test_folded_function_after_caller_is_found(self):
        # defines() scans the whole module: the fold must be found no
        # matter which define comes first in the file.
        clusters, skipped = im.inline_map(module(CALLER, LEAF), module(CALLER))
        self.assertEqual(clusters, {"caller": ["leaf"]})
        self.assertEqual(skipped, [])

    def test_clang_folded_function_maps_from_its_tu(self):
        # Clang folds TU-local single callers before the link: gone
        # from the linked module itself, one caller in its own TU.
        tu = module(CALLER, LEAF)
        linked = module(CALLER)
        clusters, skipped = im.inline_map(linked, linked, [tu])
        self.assertEqual(clusters, {"caller": ["leaf"]})
        self.assertEqual(skipped, [])

    def test_survivor_is_not_mapped(self):
        clusters, skipped = im.inline_map(module(LEAF, CALLER), module(LEAF, CALLER))
        self.assertEqual(clusters, {})
        self.assertEqual(skipped, [])

    def test_multi_caller_fold_is_skipped(self):
        two = """define void @other() {
  call void @leaf()
  ret void
}"""
        clusters, skipped = im.inline_map(
            module(LEAF, CALLER, two), module(CALLER, two)
        )
        self.assertEqual(clusters, {})
        self.assertEqual(skipped, [("leaf", ["caller", "other"])])

    def test_uncalled_vanished_function_is_skipped(self):
        clusters, skipped = im.inline_map(module(LEAF), "")
        self.assertEqual(clusters, {})
        self.assertEqual(skipped, [("leaf", [])])

    def test_llvm_intrinsics_are_not_functions(self):
        intrinsic = """define void @llvm.trap() {
  ret void
}"""
        user = """define void @user() {
  call void @llvm.trap()
  ret void
}"""
        clusters, skipped = im.inline_map(
            module(intrinsic, user), module(intrinsic, user)
        )
        self.assertEqual(clusters, {})
        self.assertEqual(skipped, [])


if __name__ == "__main__":
    unittest.main()
