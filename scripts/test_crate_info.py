#!/usr/bin/env python3
"""Tests for crate_info.py; run with `python3 -I scripts/test_crate_info.py`."""

import contextlib
import io
import json
import pathlib
import sys
import tempfile
import unittest
import urllib.error
from unittest import mock

sys.dont_write_bytecode = True
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import crate_info  # noqa: E402


class IndexPath(unittest.TestCase):
    def test_shards_by_length(self) -> None:
        cases = {
            "a": "1/a",
            "ab": "2/ab",
            "abc": "3/a/abc",
            "abcd": "ab/cd/abcd",
            "craftainer-crud": "cr/af/craftainer-crud",
        }
        for name, expected in cases.items():
            with self.subTest(name=name):
                self.assertEqual(crate_info.index_path(name), expected)

    def test_whole_name_is_lowercased(self) -> None:
        self.assertEqual(crate_info.index_path("MyCrate"), "my/cr/mycrate")
        self.assertEqual(crate_info.index_path("AB"), "2/ab")
        self.assertEqual(crate_info.index_path("ABC"), "3/a/abc")


class Manifest(unittest.TestCase):
    def test_name_override_order(self) -> None:
        manifest = {"package": {"name": "from-toml"}}
        self.assertEqual(crate_info.crate_name(manifest, {}), "from-toml")
        self.assertEqual(crate_info.crate_name(manifest, {"CARGO_PACKAGE": "pkg"}), "pkg")
        self.assertEqual(crate_info.crate_name(manifest, {"CARGO_PACKAGE": "pkg", "CRATE": "c"}), "c")

    def test_version_sources(self) -> None:
        literal = {"package": {"version": "1.2.3"}}
        inherited = {"package": {"version": {"workspace": True}}, "workspace": {"package": {"version": "0.0.0"}}}
        virtual = {"workspace": {"package": {"version": "0.4.0"}}}
        self.assertEqual(crate_info.crate_version(literal, {}), "1.2.3")
        self.assertEqual(crate_info.crate_version(inherited, {}), "0.0.0")
        self.assertEqual(crate_info.crate_version(virtual, {}), "0.4.0")
        self.assertEqual(crate_info.crate_version(inherited, {"RELEASE_VERSION": "9.9.9"}), "9.9.9")

    def test_index_url_base(self) -> None:
        self.assertEqual(crate_info.index_url("MyCrate", {}), "https://index.crates.io/my/cr/mycrate")
        env = {"CRATES_INDEX_URL": "file:///x/"}
        self.assertEqual(crate_info.index_url("abc", env), "file:///x/3/a/abc")


class IndexFixture(unittest.TestCase):
    """A file:// sparse index holding one crate (MyCrate) with the given body."""

    def url(self, body: "str | None") -> str:
        root = tempfile.TemporaryDirectory()
        self.addCleanup(root.cleanup)
        if body is not None:
            path = pathlib.Path(root.name, crate_info.index_path("MyCrate"))
            path.parent.mkdir(parents=True)
            path.write_text(body)
        return crate_info.index_url("MyCrate", {"CRATES_INDEX_URL": pathlib.Path(root.name).as_uri()})


def lines(*entries: dict) -> str:
    return "\n".join(json.dumps(e) for e in entries) + "\n"


GOOD = lines(
    {"vers": "1.0.0", "yanked": False, "cksum": "aa"},
    {"vers": "1.1.0", "yanked": True, "cksum": "bb"},
)


class HasVersion(IndexFixture):
    def test_present_missing_and_yanked(self) -> None:
        url = self.url(GOOD)
        self.assertEqual(crate_info.has_version(url, "1.0.0", False), 0)
        self.assertEqual(crate_info.has_version(url, "2.0.0", False), 1)
        self.assertEqual(crate_info.has_version(url, "1.1.0", False), 0)
        self.assertEqual(crate_info.has_version(url, "1.1.0", True), 1)

    def test_unknown_crate_is_not_published(self) -> None:
        self.assertEqual(crate_info.has_version(self.url(None), "1.0.0", False), 1)

    def test_malformed_bodies_are_index_failures(self) -> None:
        bad_bodies = {
            "html": "<html><body>Service Unavailable</body></html>\n",
            "truncated": GOOD + '{"vers":"1.2.0","yank',
            "missing vers": lines({"yanked": False}),
            "missing yanked": lines({"vers": "1.0.0"}),
            "not an object": "[1, 2]\n",
        }
        for label, body in bad_bodies.items():
            with self.subTest(label), contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(crate_info.has_version(self.url(body), "1.0.0", False), 2)

    def test_http_errors(self) -> None:
        error = urllib.error.HTTPError("u", 503, "x", None, None)
        with mock.patch("urllib.request.urlopen", side_effect=error), contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(crate_info.has_version("https://x", "1.0.0", False), 2)
        gone = urllib.error.HTTPError("u", 404, "x", None, None)
        with mock.patch("urllib.request.urlopen", side_effect=gone):
            self.assertEqual(crate_info.has_version("https://x", "1.0.0", False), 1)
        down = urllib.error.URLError("connection refused")
        with mock.patch("urllib.request.urlopen", side_effect=down), contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(crate_info.has_version("https://x", "1.0.0", False), 2)


class Checksum(IndexFixture):
    def test_listed_unlisted_and_unknown(self) -> None:
        url = self.url(GOOD)
        self.assertEqual(crate_info.checksum(url, "1.0.0"), (0, "aa"))
        self.assertEqual(crate_info.checksum(url, "9.9.9"), (1, ""))
        self.assertEqual(crate_info.checksum(self.url(None), "1.0.0"), (1, ""))

    def test_failures(self) -> None:
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(crate_info.checksum(self.url("<html>\n"), "1.0.0"), (2, ""))
            nocksum = self.url(lines({"vers": "1.0.0", "yanked": False}))
            self.assertEqual(crate_info.checksum(nocksum, "1.0.0"), (2, ""))


class Cli(unittest.TestCase):
    def test_usage_errors_exit_64(self) -> None:
        for argv in (
            [],
            ["bogus"],
            ["has-version"],
            ["has-version", "1.0.0", "--bogus"],
            ["has-version", "1.0.0", "--not-yanked", "extra"],
            ["checksum"],
            ["name", "extra"],
            ["target-dir", "extra"],
        ):
            with self.subTest(argv), contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(crate_info.main(argv), 64)


class TargetDir(unittest.TestCase):
    def test_reads_target_directory(self) -> None:
        run = mock.Mock(return_value=mock.Mock(returncode=0, stdout='{"target_directory": "/x/t"}'))
        self.assertEqual(crate_info.target_directory(run), "/x/t")
        self.assertEqual(run.call_args.args[0][:2], ["cargo", "metadata"])

    def test_cargo_failure_exits_with_the_cause(self) -> None:
        run = mock.Mock(return_value=mock.Mock(returncode=101, stderr="no manifest"))
        with self.assertRaises(SystemExit) as raised:
            crate_info.target_directory(run)
        self.assertIn("no manifest", str(raised.exception))


if __name__ == "__main__":
    unittest.main()
