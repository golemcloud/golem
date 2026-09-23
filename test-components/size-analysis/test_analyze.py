import itertools
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from analyze import (
    COMPONENT,
    CORE,
    attribution,
    custom_name,
    inspect,
    preserve_component_types,
    read_u32,
    rewrite_cores,
    sections,
    u32,
)
from retention import PENDING_B_SYMBOLS, evaluate_retention


def section(kind, payload):
    return bytes([kind]) + u32(len(payload)) + payload


def custom(name, payload):
    name = name.encode()
    return section(0, u32(len(name)) + name + payload)


class BinaryTests(unittest.TestCase):
    def test_leb_boundaries_and_invalid_lengths(self):
        for value in (0, 127, 128, 16383, 16384, 0xFFFFFFFF):
            encoded = u32(value)
            self.assertEqual(read_u32(encoded, 0), (value, len(encoded)))
        for data in (b"", b"\x80", b"\xff\xff\xff\xff\x1f", b"\x80" * 5):
            with self.assertRaises(ValueError):
                read_u32(data, 0)
        with self.assertRaises(ValueError):
            list(sections(CORE + b"\x00\x7fabc"))

    def test_nested_sections_account_for_every_byte_without_double_counting(self):
        code = section(10, b"\x01\x02\x00\x0b")
        data = section(11, b"\x00")
        module = CORE + code + data + custom("name", b"x" * 130)
        nested = COMPONENT + section(1, module)
        component = COMPONENT + section(4, nested) + section(1, CORE + data)
        rows = inspect(component)
        self.assertEqual(sum(row["bytes"] for row in rows), len(component))
        self.assertEqual(
            sum(row["bytes"] for row in rows if row["section"] == "code"), 6
        )
        self.assertEqual(
            sum(row["bytes"] for row in rows if row["section"] == "data"), 6
        )
        for left, right in itertools.pairwise(rows):
            self.assertEqual(left["offset"] + left["bytes"], right["offset"])

    def test_rebundle_updates_nested_lengths_preserving_wrapper_and_core_order(self):
        first = CORE + custom("first", b"a")
        second = CORE + custom("second", b"b" * 128)
        metadata = custom("component-name", b"wrapper")
        nested = COMPONENT + section(1, second) + metadata
        original = COMPONENT + section(1, first) + section(4, nested) + metadata
        self.assertEqual(rewrite_cores(original, lambda core: core), original)
        visited = []

        def shrink(core):
            visited.append(core)
            return CORE

        result = rewrite_cores(original, shrink)
        self.assertEqual(visited, [first, second])
        expected = (
            COMPONENT
            + section(1, CORE)
            + section(4, COMPONENT + section(1, CORE) + metadata)
            + metadata
        )
        self.assertEqual(result, expected)

    def test_component_type_metadata_restored_once_without_stale_names(self):
        types = custom("component-type:a", b"a" * 137) + custom(
            "component-type:b", b"b"
        )
        original = CORE + types + custom("name", b"old")
        optimized = (
            CORE + custom("component-type:a", b"changed") + custom("name", b"new")
        )
        restored = preserve_component_types(original, optimized)
        self.assertEqual(restored, CORE + custom("name", b"new") + types)
        self.assertEqual(preserve_component_types(original, restored), restored)
        names = [
            custom_name(restored[payload:end])
            for kind, _, payload, end in sections(restored)
            if kind == 0
        ]
        self.assertEqual(names, ["name", "component-type:a", "component-type:b"])

    def test_attribution_handles_rust_v0_disambiguators_and_unknown_symbols(self):
        symbols = [
            {"name": "golem_rust[abc012]::agent::invoke", "shallow_size": 73},
            {"name": "<golem_rust::Type as core::Trait>::invoke", "shallow_size": 19},
            {"name": "core[def34]::fmt", "shallow_size": 41},
            {"name": "data segment .data", "shallow_size": 103},
        ]
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            with (
                patch("analyze.shutil.which", return_value="twiggy"),
                patch("analyze.run", side_effect=[b"", json.dumps(symbols).encode()]),
            ):
                attribution(directory / "module.wasm", directory)
            self.assertEqual(
                dict(json.loads((directory / "crate-estimates.json").read_text())),
                {"golem_rust": 92, "core": 41, "[unattributed]": 103},
            )

    def test_retention_contract_separates_current_and_option_b_expectations(self):
        report = evaluate_retention(
            {
                "tool": {
                    "wat": '(import "golem:agent/host" "make-agent-id")',
                    "symbols": "\n".join(PENDING_B_SYMBOLS),
                }
            },
            {
                "reflection": {
                    "wat": '(import "golem:agent/host" "get-agent-type")',
                    "symbols": "retention_reflection::has_agent_type",
                }
            },
        )
        self.assertEqual(
            report["negative"]["tool"]["pendingOptionBSymbolsStillRetained"],
            list(PENDING_B_SYMBOLS),
        )
        self.assertEqual(
            report["positive"]["reflection"]["expectedReflectionImports"],
            ["get-agent-type"],
        )

    def test_retention_contract_rejects_wrong_fixture_polarity(self):
        with self.assertRaisesRegex(AssertionError, "retains reflection imports"):
            evaluate_retention(
                {
                    "tool": {
                        "wat": '(import "golem:tool/host" "get-tool-type" (func))',
                        "symbols": "",
                    }
                },
                {
                    "reflection": {
                        "wat": '(import "golem:agent/host" "get-agent-type" (func))',
                        "symbols": "has_agent_type",
                    }
                },
            )
        with self.assertRaisesRegex(AssertionError, "does not retain get-agent-type"):
            evaluate_retention(
                {"tool": {"wat": "", "symbols": ""}},
                {"reflection": {"wat": "", "symbols": ""}},
            )
        with self.assertRaisesRegex(AssertionError, "retains option B symbols"):
            evaluate_retention(
                {"tool": {"wat": "", "symbols": "regex_automata::dfa"}},
                {
                    "reflection": {
                        "wat": '(import "golem:agent/host" "get-agent-type" (func))',
                        "symbols": "has_agent_type",
                    }
                },
                enforce_option_b=True,
            )

        report = evaluate_retention(
            {
                "tool": {
                    "wat": '(module (func (export "get-agent-type")))',
                    "symbols": "",
                }
            },
            {
                "reflection": {
                    "wat": '(module (import "golem:agent/host" "get-agent-type" (func)))',
                    "symbols": "has_agent_type",
                }
            },
        )
        self.assertEqual(report["negative"]["tool"]["forbiddenReflectionImports"], [])

    def test_retention_contract_ignores_same_named_imports_from_unrelated_interfaces(self):
        report = evaluate_retention(
            {
                "tool": {
                    "wat": '(module (import "example:unrelated/host" "get-agent-type" (func)))',
                    "symbols": "",
                }
            },
            {
                "reflection": {
                    "wat": '(module (import "golem:agent/host" "get-agent-type" (func)))',
                    "symbols": "has_agent_type",
                }
            },
        )
        self.assertEqual(report["negative"]["tool"]["forbiddenReflectionImports"], [])


if __name__ == "__main__":
    unittest.main()
