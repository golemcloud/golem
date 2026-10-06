#!/usr/bin/env python3
"""List third-party Cargo packages that expose configurable feature flags."""

from __future__ import annotations

import argparse
import json
import subprocess
from pathlib import Path
from typing import Any


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--out",
        type=Path,
        default=Path("tmp/cargo-dependency-features.md"),
        help="output Markdown file (default: %(default)s)",
    )
    parser.add_argument(
        "--metadata",
        type=Path,
        help="use existing cargo metadata JSON instead of invoking Cargo",
    )
    parser.add_argument(
        "--filter-platform",
        help="pass --filter-platform TARGET to cargo metadata",
    )
    return parser.parse_args()


def load_metadata(args: argparse.Namespace) -> dict[str, Any]:
    if args.metadata:
        return json.loads(args.metadata.read_text())

    command = ["cargo", "metadata", "--locked", "--format-version", "1"]
    if args.filter_platform:
        command.extend(["--filter-platform", args.filter_platform])
    return json.loads(subprocess.check_output(command))


def is_first_party(package: dict[str, Any], workspace_root: Path) -> bool:
    return Path(package["manifest_path"]).resolve().is_relative_to(workspace_root)


def source_label(package: dict[str, Any]) -> str:
    source = package.get("source") or "path"
    if source.startswith("registry+"):
        return "registry"
    if source.startswith("git+"):
        return "git"
    return "path"


def build_feature_records(metadata: dict[str, Any]) -> tuple[list[dict[str, Any]], int]:
    workspace_root = Path(metadata["workspace_root"]).resolve()
    packages = {package["id"]: package for package in metadata["packages"]}
    resolve_nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    reverse_dependents: dict[str, set[str]] = {package_id: set() for package_id in packages}
    workspace_consumers: dict[str, set[str]] = {
        package_id: set() for package_id in packages
    }

    for node in metadata["resolve"]["nodes"]:
        dependent = packages[node["id"]]
        dependent_is_first_party = is_first_party(dependent, workspace_root)
        for dependency in node.get("deps", []):
            dependency_id = dependency["pkg"]
            reverse_dependents.setdefault(dependency_id, set()).add(dependent["id"])
            if dependent_is_first_party:
                workspace_consumers.setdefault(dependency_id, set()).add(dependent["name"])

    third_party_count = 0
    records = []
    for package in packages.values():
        if is_first_party(package, workspace_root):
            continue
        third_party_count += 1
        available = sorted(
            feature for feature in package.get("features", {}) if feature != "default"
        )
        if not available:
            continue

        enabled_features = set(resolve_nodes.get(package["id"], {}).get("features", []))
        records.append(
            {
                "name": package["name"],
                "version": package["version"],
                "source": source_label(package),
                "default_enabled": "default" in enabled_features,
                "enabled": sorted(enabled_features.intersection(available)),
                "disabled": sorted(set(available).difference(enabled_features)),
                "feature_count": len(available),
                "reverse_dependent_count": len(reverse_dependents.get(package["id"], set())),
                "workspace_consumers": sorted(workspace_consumers.get(package["id"], set())),
            }
        )

    records.sort(key=lambda record: (record["name"], record["version"], record["source"]))
    return records, third_party_count


def markdown_code_list(values: list[str]) -> str:
    if not values:
        return "—"
    return "<br>".join(f"`{value.replace('|', '&#124;')}`" for value in values)


def render_report(
    records: list[dict[str, Any]],
    third_party_count: int,
    platform: str | None,
) -> str:
    defaults_enabled = sum(record["default_enabled"] for record in records)
    enabled_non_default = sum(bool(record["enabled"]) for record in records)
    lines = [
        "# Cargo dependency feature audit",
        "",
        "Generated from `cargo metadata --locked --format-version 1`.",
        "The scope is every resolved third-party package in the root workspace, including normal, build, dev, and target-specific dependencies.",
    ]
    if platform:
        lines.append(f"Cargo metadata was filtered for `{platform}`.")
    else:
        lines.append(
            "No platform filter was applied, so target-specific packages for all platforms can appear."
        )
    lines.extend(
        [
            "Feature state is Cargo's workspace-wide unified resolution, not the state for one binary.",
            "",
            f"- Resolved third-party packages: **{third_party_count}**",
            f"- Packages with at least one non-`default` feature: **{len(records)}**",
            f"- Of those, packages with `default` enabled: **{defaults_enabled}**",
            f"- Of those, packages with an enabled non-default feature: **{enabled_non_default}**",
            "",
            "`Direct workspace users` is populated only when a first-party crate directly depends on the package; transitive packages normally show `—`.",
            "",
            "| package | source | default enabled | enabled non-default features | disabled non-default features | direct dependents | direct workspace users |",
            "|---|---|:---:|---|---|---:|---|",
        ]
    )
    for record in records:
        package = f"`{record['name']} {record['version']}` ({record['feature_count']} flags)"
        lines.append(
            "| "
            + " | ".join(
                [
                    package,
                    record["source"],
                    "yes" if record["default_enabled"] else "no",
                    markdown_code_list(record["enabled"]),
                    markdown_code_list(record["disabled"]),
                    str(record["reverse_dependent_count"]),
                    markdown_code_list(record["workspace_consumers"]),
                ]
            )
            + " |"
        )
    lines.append("")
    return "\n".join(lines)


def main() -> None:
    args = parse_args()
    metadata = load_metadata(args)
    records, third_party_count = build_feature_records(metadata)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(
        render_report(records, third_party_count, args.filter_platform), encoding="utf-8"
    )
    print(
        f"wrote {args.out}: {len(records)} of {third_party_count} third-party packages "
        "have non-default feature flags"
    )


if __name__ == "__main__":
    main()
