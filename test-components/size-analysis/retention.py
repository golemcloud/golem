#!/usr/bin/env python3
"""Check reflection retention contracts in named, unstripped WASM components."""

import argparse
import json
import re
import tempfile
from pathlib import Path

from analyze import sections, rewrite_cores, run


REFLECTION_IMPORTS = ("get-agent-type", "get-all-agent-types", "get-tool-type")
REFLECTION_IMPORT_RE = re.compile(
    r'\(import\s+"([^"]+)"\s+"('
    + "|".join(re.escape(name) for name in REFLECTION_IMPORTS)
    + r')"'
)
REFLECTION_IMPORT_INTERFACES = {
    "get-agent-type": "golem:agent/host",
    "get-all-agent-types": "golem:agent/host",
    "get-tool-type": "golem:tool/host",
}
PENDING_B_SYMBOLS = (
    "golem_rust::agentic::reflection",
    "golem_rust::agentic::tool_reflection",
    "golem_schema::schema::render",
    "golem_schema::schema::validation",
    "golem_schema::schema::well_formedness",
    "regex_automata",
    "url::",
    "idna",
    "serde_json::value",
)


def retained_patterns(text, patterns):
    normalized = re.sub(r"([A-Za-z_][A-Za-z0-9_]*)\[[0-9a-f]+\]", r"\1", text)
    return [pattern for pattern in patterns if pattern in normalized]


def retained_reflection_imports(wat):
    imported = {
        name
        for interface, name in REFLECTION_IMPORT_RE.findall(wat)
        if interface.split("@", 1)[0] == REFLECTION_IMPORT_INTERFACES[name]
    }
    return [name for name in REFLECTION_IMPORTS if name in imported]


def evaluate_retention(negative, positive, enforce_option_b=False):
    failures = []
    report = {"negative": {}, "positive": {}}
    for label, evidence in negative.items():
        imports = retained_reflection_imports(evidence["wat"])
        pending = retained_patterns(evidence["symbols"], PENDING_B_SYMBOLS)
        report["negative"][label] = {
            "forbiddenReflectionImports": imports,
            "pendingOptionBNegativeAssertions": list(PENDING_B_SYMBOLS),
            "pendingOptionBSymbolsStillRetained": pending,
        }
        if imports:
            failures.append(f"{label} retains reflection imports: {', '.join(imports)}")
        if enforce_option_b and pending:
            failures.append(f"{label} retains option B symbols: {', '.join(pending)}")
    for label, evidence in positive.items():
        imports = retained_reflection_imports(evidence["wat"])
        symbols = retained_patterns(evidence["symbols"], ("has_agent_type",))
        report["positive"][label] = {
            "expectedReflectionImports": imports,
            "expectedReflectionSymbols": symbols,
        }
        if "get-agent-type" not in imports:
            failures.append(f"{label} does not retain get-agent-type")
        if "has_agent_type" not in symbols:
            failures.append(f"{label} does not retain the explicit reflection fixture symbol")
    if failures:
        raise AssertionError("; ".join(failures))
    return report


def component_evidence(component):
    evidence = {"wat": "", "symbols": ""}
    with tempfile.TemporaryDirectory() as temporary:
        directory = Path(temporary)
        cores = []

        def collect(core):
            cores.append(core)
            return core

        rewrite_cores(component.read_bytes(), collect)
        for index, core in enumerate(cores):
            raw = directory / f"core-{index}.wasm"
            demangled = directory / f"core-{index}-demangled.wasm"
            raw.write_bytes(core)
            evidence["wat"] += run(["wasm-tools", "print", raw]).decode(
                errors="replace"
            )
            if not any(kind == 10 for kind, _, _, _ in sections(core)):
                continue
            run(["wasm-tools", "demangle", raw, "-o", demangled])
            symbols = json.loads(
                run(["twiggy", "top", "-n", "1000000", "--format", "json", demangled])
            )
            evidence["symbols"] += "\n".join(item["name"] for item in symbols)
    return evidence


def labeled_path(value):
    label, separator, path = value.partition("=")
    if not separator or not label or not path:
        raise argparse.ArgumentTypeError("expected LABEL=PATH")
    return label, Path(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--negative", action="append", default=[], type=labeled_path)
    parser.add_argument("--positive", action="append", default=[], type=labeled_path)
    parser.add_argument("--enforce-option-b", action="store_true")
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    if not args.negative or not args.positive:
        parser.error("at least one --negative and one --positive fixture are required")
    report = evaluate_retention(
        {label: component_evidence(path) for label, path in args.negative},
        {label: component_evidence(path) for label, path in args.positive},
        enforce_option_b=args.enforce_option_b,
    )
    rendered = json.dumps(report, indent=2) + "\n"
    if args.out:
        args.out.write_text(rendered)
    print(rendered, end="")


if __name__ == "__main__":
    main()
