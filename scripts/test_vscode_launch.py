"""The VS Code launch appendix keeps a working copy-paste snippet (epic-cc#1360).

docs/internals/design/34-debugger-design.md carries a `launch.json` for
attaching to `epic-cc-gdbserver` through `cppdbg`. The snippet must stay
valid JSON, and its command strings must keep matching the session the
driver and server actually accept: `miDebuggerServerAddress` for the
attach, `target remote` for the server address, `file` for the sidecar
ELF, and the `epic-cc --sidecar` / `epic-cc-gdbserver` invocations that
produce and serve it. Run by scripts/ci-test.sh alongside the cargo suites.
"""

import json
import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
DESIGN = ROOT / "docs" / "internals" / "design" / "34-debugger-design.md"

APPENDIX = "## Appendix: debugging in VS Code"
COMMANDS = (
    "epic-cc-gdbserver",
    "--sidecar",
    "miDebuggerServerAddress",
    "target remote",
)


def appendix_text():
    text = DESIGN.read_text()
    start = text.index(APPENDIX)
    return text[start:]


def launch_json():
    body = appendix_text()
    m = re.search(r"```json\n(.*?)```", body, re.DOTALL)
    assert m is not None, "appendix has no ```json block"
    return json.loads(m.group(1))


class LaunchSnippetTests(unittest.TestCase):
    def test_commands_present(self):
        body = appendix_text()
        for cmd in COMMANDS:
            self.assertIn(cmd, body, f"appendix lost {cmd!r}")

    def test_snippet_parses_as_json(self):
        doc = launch_json()
        (config,) = doc["configurations"]
        self.assertEqual(config["type"], "cppdbg")
        self.assertIn("127.0.0.1", config["miDebuggerServerAddress"])

    def test_snippet_loads_sidecar_and_targets_server(self):
        doc = launch_json()
        (config,) = doc["configurations"]
        setup = " ".join(c["text"] for c in config["setupCommands"])
        self.assertIn("target remote", setup)
        self.assertIn("file ", setup)
        self.assertIn(".elf", setup)
        self.assertIn(".elf", config["program"])


if __name__ == "__main__":
    unittest.main()
