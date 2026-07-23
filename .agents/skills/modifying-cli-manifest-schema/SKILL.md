---
name: modifying-cli-manifest-schema
description: "Adding or changing application manifest JSON schema versions and aligning CLI schema references."
---

# Skill: modifying-cli-manifest-schema

Use this skill when changing the Golem application manifest JSON schema under
`cli/schema.golem.cloud/app/golem/` or when adding/removing manifest fields in
`cli/golem-cli` that must be reflected in schema validation and generated
template references.

Do **not** use this skill for the structured command output schema under
`cli/golem-cli/command-output-schema/command-output.schema.json`. For `CliOutput`
types, `to_cli_output_value`, or command-output schema generators, use
`modifying-cli-output-schema` instead.

## Core Rules

1. Do not edit the currently published schema version in place for feature work.
2. Create exactly one new schema version directory per PR.
3. Copy the latest schema directory as the starting point for the new version.
4. Apply schema changes only in the new version directory.
5. Update the CLI to point to the new schema version when the new schema should
   become the default for generated manifests and validation.

## Important Terminology

- **Manifest version**: the version of the YAML document itself, exposed as
  `sdk::MANIFEST` in `cli/golem-cli/src/versions.rs`.
- **Manifest schema version**: the version of the JSON schema under
  `cli/schema.golem.cloud/app/golem/<version>/golem.schema.json`, exposed via
  `manifest_schema_version!()` in `cli/golem-cli/src/versions.rs`.

These are NOT the same concept and do not have to move together.

Examples:

- A schema-only bug fix may create `1.5.1.1` without changing the manifest
  version.
- A new development-line schema may create `1.6.0-dev.1` while the manifest
  version is a release-line version such as `1.6.0`.

## Choosing the New Version

Use project/release direction from the user or surrounding work. If not stated:

- Use a new `-dev.N` schema version for in-progress development line work.
- Use a patch-like schema version only for schema-only fixes intended to refine
  an already established line.

Schema versions are published to schema hosting during development, so `-dev.N`
versions are useful and expected. Manifest document versions are release-line
versions and may accumulate multiple in-tree changes before release.

For this repository, the user has stated the convention that we usually create
one new schema version per PR.

## Workflow

1. Identify the current latest schema directory under
   `cli/schema.golem.cloud/app/golem/`.
2. Create the requested new directory by copying the latest schema version.
3. Modify only the new copied schema.
4. Update `cli/golem-cli/src/versions.rs` carefully:
   - `sdk::MANIFEST` only if the YAML document version itself should change.
   - `manifest_schema_version!()` to the new schema version if the CLI should
     validate against and emit references to the new schema by default.
   - If `sdk::MANIFEST` changes, update manifest-version compatibility policy
     and tests in `cli/golem-cli/src/app/manifest_version.rs`.
5. Check other schema-version consumers, especially:
   - `cli/golem-cli/src/lib.rs`
   - `cli/golem-cli/src/app/template/snippet.rs`
   - tests containing embedded `$schema` references
6. Update tests as needed.
7. Run focused validation/build/tests.

## Things To Watch

- Do not mass-edit old schema versions unless the user explicitly wants a backfill.
- Do not assume `sdk::MANIFEST` and `manifest_schema_version!()` should always match.
- Do not bump `sdk::MANIFEST` without checking whether existing manifest
  versions should remain compatible.
- When introducing a new field or enum value, ensure both serde parsing and JSON
  schema validation agree.
- If the CLI emits manifest templates/snippets, make sure they reference the new
  schema version.

## Useful Files

- `cli/golem-cli/src/versions.rs`
- `cli/golem-cli/src/lib.rs`
- `cli/golem-cli/src/app/template/snippet.rs`
- `cli/golem-cli/src/model/app_raw.rs`
- `cli/schema.golem.cloud/app/golem/*/golem.schema.json`

## Verification Checklist

- New schema directory exists and was copied from the intended predecessor.
- The new schema validates the new manifest feature.
- Old schema directories were not modified unless explicitly intended.
- CLI version constants were updated correctly.
- `cargo check -p golem-cli` passes.
- Relevant tests pass.
