#!/usr/bin/env python3
"""Self-tests for the conformance harness's exact result comparison."""

import importlib.util
import pathlib
import subprocess
import tempfile
import unittest


SCRIPT = pathlib.Path(__file__).with_name("conformance.py")
SPEC = importlib.util.spec_from_file_location("conformance", SCRIPT)
HARNESS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(HARNESS)


class ExactOutcomeTests(unittest.TestCase):
    def completed(self, stdout=b"", stderr=b"", returncode=0):
        return subprocess.CompletedProcess([], returncode, stdout, stderr)

    def test_exact_status_stdout_and_stderr_pass(self):
        result = self.completed(stdout=b"out\n", stderr=b"err\n", returncode=7)
        self.assertTrue(
            HARNESS.outcomes_match((7, b"out\n", b"err\n"), HARNESS.outcome(result))
        )

    def test_missing_expected_stderr_fails(self):
        result = self.completed(stdout=b"out\n")
        self.assertFalse(
            HARNESS.outcomes_match(
                (0, b"out\n", b"required diagnostic\n"), HARNESS.outcome(result)
            )
        )

    def test_unexpected_actual_stderr_fails(self):
        result = self.completed(stdout=b"out\n", stderr=b"surprise\n")
        self.assertFalse(
            HARNESS.outcomes_match((0, b"out\n", b""), HARNESS.outcome(result))
        )

    def test_wasm_report_preserves_nonzero_shell_status(self):
        with tempfile.TemporaryDirectory() as directory:
            status = pathlib.Path(directory) / "status"
            status.write_text("2", encoding="ascii")
            result = self.completed(stderr=b"refused\n", returncode=1)
            self.assertEqual(
                (2, b"", b"refused\n"), HARNESS.wasm_outcome(result, status)
            )

    def test_wrong_shell_status_fails_even_when_wasi_status_matches(self):
        with tempfile.TemporaryDirectory() as directory:
            status = pathlib.Path(directory) / "status"
            status.write_text("1", encoding="ascii")
            result = self.completed(stderr=b"refused\n", returncode=1)
            self.assertFalse(
                HARNESS.outcomes_match(
                    (2, b"", b"refused\n"), HARNESS.wasm_outcome(result, status)
                )
            )

    def test_missing_report_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(ValueError):
                HARNESS.wasm_outcome(
                    self.completed(), pathlib.Path(directory) / "missing"
                )

    def test_runtime_failure_after_report_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            status = pathlib.Path(directory) / "status"
            status.write_text("0", encoding="ascii")
            with self.assertRaises(ValueError):
                HARNESS.wasm_outcome(self.completed(returncode=1), status)



def corpus(directory, name, text):
    path = pathlib.Path(directory) / f"{name}.py"
    path.write_text(text, encoding="utf-8")
    return pathlib.Path(directory)


class CorpusTests(unittest.TestCase):
    def test_tags_merge_module_defaults_with_case_tags(self):
        with tempfile.TemporaryDirectory() as directory:
            root = corpus(directory, "a", 'TAGS = ["cmd.x"]\nCASES = [("a1", "true", ["syntax.y"]), ("a2", "true")]\n')
            cases = HARNESS.load_cases(root)
            self.assertEqual(("cmd.x", "syntax.y"), cases[0].tags)
            self.assertEqual(("cmd.x",), cases[1].tags)

    def test_fixture_without_reason_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = corpus(directory, "a", 'CASES = [("a1", "true")]\nEXPECTED = {"a1": (0, b"", b"")}\n')
            with self.assertRaisesRegex(ValueError, "needs a reason"):
                HARNESS.load_cases(root)

    def test_module_reason_covers_its_fixtures(self):
        with tempfile.TemporaryDirectory() as directory:
            root = corpus(
                directory, "a",
                'CASES = [("a1", "true")]\nEXPECTED_REASON = "why"\nEXPECTED = {"a1": (0, b"", b"")}\n',
            )
            self.assertEqual((0, b"", b"", "why"), HARNESS.load_cases(root)[0].deliberate)

    def test_fixture_for_an_unknown_case_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = corpus(directory, "a", 'CASES = [("a1", "true")]\nEXPECTED = {"typo": (0, b"", b"", "r")}\n')
            with self.assertRaisesRegex(ValueError, "names no case"):
                HARNESS.load_cases(root)

    def test_duplicate_names_across_corpora_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            corpus(directory, "a", 'CASES = [("same", "true")]\n')
            root = corpus(directory, "b", 'CASES = [("same", "false")]\n')
            with self.assertRaisesRegex(ValueError, "duplicate"):
                HARNESS.load_cases(root)


class GoldenTests(unittest.TestCase):
    def case(self, script="echo hi"):
        return HARNESS.Case("c", script, "corpus")

    def test_outcomes_round_trip_including_invalid_utf8(self):
        result = (3, b"text\n", b"\xff\xfe raw")
        entry = HARNESS.encode_outcome(self.case(), result)
        self.assertEqual({"text": "text\n"}, entry["stdout"])
        self.assertIn("base64", entry["stderr"])
        self.assertEqual(result, HARNESS.decode_outcome(entry))

    def test_golden_for_an_edited_script_is_stale(self):
        fingerprint = {"dockerfile": "d", "profile": "p"}
        entry = HARNESS.encode_outcome(self.case("echo old"), (0, b"", b""))
        loaded = {"corpus": {"oracle": fingerprint, "cases": {"c": entry}}}
        golden, error = HARNESS.golden_for(self.case("echo new"), loaded, fingerprint)
        self.assertIsNone(golden)
        self.assertIn("older script", error)

    def test_goldens_from_another_oracle_are_unusable(self):
        entry = HARNESS.encode_outcome(self.case(), (0, b"", b""))
        loaded = {"corpus": {"oracle": {"dockerfile": "old", "profile": "p"}, "cases": {"c": entry}}}
        golden, error = HARNESS.golden_for(self.case(), loaded, {"dockerfile": "new", "profile": "p"})
        self.assertIsNone(golden)
        self.assertIn("another oracle", error)

    def test_missing_golden_is_reported(self):
        fingerprint = {"dockerfile": "d", "profile": "p"}
        loaded = {"corpus": {"oracle": fingerprint, "cases": {}}}
        self.assertIn("no golden", HARNESS.golden_for(self.case(), loaded, fingerprint)[1])

    def test_fixture_matching_the_oracle_is_redundant(self):
        case = self.case()
        case.deliberate = (0, b"hi\n", b"", "reason")
        self.assertIsNotNone(HARNESS.redundant_fixture(case, (0, b"hi\n", b"")))
        self.assertIsNone(HARNESS.redundant_fixture(case, (0, b"other\n", b"")))

    def test_stderr_fixture_replaces_only_stderr(self):
        case = self.case()
        case.deliberate_stderr = (b"ours\n", "reason")
        self.assertEqual((1, b"out", b"ours\n"), HARNESS.expected_for(case, (1, b"out", b"theirs\n")))


class CallTests(unittest.TestCase):
    def test_a_script_without_markers_is_one_call_in_the_root(self):
        self.assertEqual([("/", "echo a\necho b")], HARNESS.split_calls("echo a\necho b"))

    def test_markers_split_calls_and_name_where_the_next_starts(self):
        script = "cd /tmp\n#--call-- /tmp\npwd\n#--call--\npwd"
        self.assertEqual(
            [("/", "cd /tmp"), ("/tmp", "pwd"), ("/", "pwd")], HARNESS.split_calls(script)
        )

    def test_a_marker_needs_its_own_line(self):
        script = "echo '#--call--'\n  #--call--\n#--call--x"
        self.assertEqual([("/", script)], HARNESS.split_calls(script))


if __name__ == "__main__":
    unittest.main()
