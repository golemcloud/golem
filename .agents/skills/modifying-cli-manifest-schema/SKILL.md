---
name: modifying-cli-manifest-schema
description: "Changing the current application manifest JSON schema and aligning CLI schema references."
---

# Modifying the CLI Manifest Schema

Use this skill when changing the Golem application manifest JSON schema under
`cli/schema.golem.cloud/app/golem/` or when adding/removing manifest fields in
`cli/golem-cli` that must be reflected in schema validation and generated
template references.

Do **not** use this skill for the structured command output schema under
`cli/golem-cli/command-output-schema/command-output.schema.json`. For
`StructuredOutput` types, structured-output serializers, or command-output schema generators, use
`modifying-cli-output-schema` instead.

## Core Rules

1. Change the current manifest contract directly and update every in-tree parser, model, template,
   example, fixture, test, schema reference, and generated application that consumes it.
2. Do not add or preserve compatibility parsing, fallback defaults, upgrade paths, aliases,
   deprecated fields, migrations, backfills, or support for older manifest versions.
3. Edit the schema directory named by `manifest_schema_version!()` unless release or schema-hosting
   work requires a new publication identifier. A new directory is a publication decision, not a
   compatibility mechanism.
4. When a new schema identifier is required, copy the current directory, update its `$id`, make it
   the sole current schema reference, and apply the contract change there. Do not backfill
   historical schema directories.
5. If the YAML document version changes, update the CLI to accept the new current version rather
   than keeping the previous version supported.

## Important Terminology

- **Manifest version**: the version of the YAML document itself, exposed as
  `sdk::MANIFEST` in `cli/golem-cli/src/versions.rs`.
- **Manifest schema version**: the version of the JSON schema under
  `cli/schema.golem.cloud/app/golem/<version>/golem.schema.json`, exposed via
  `manifest_schema_version!()` in `cli/golem-cli/src/versions.rs`.

These are NOT the same concept and do not have to move together.

Schema hosting may use development identifiers such as `1.6.0-dev.8` while the manifest document
uses a release-line identifier such as `1.6.0`. Follow explicit release direction when choosing a
new publication identifier; do not invent a version bump merely to retain the old schema.

## Workflow

1. Read `cli/golem-cli/src/versions.rs` and identify the schema directory named by
   `manifest_schema_version!()`.
2. Modify that current schema in place, or create a new publication directory only when the task's
   release/schema-hosting requirements call for one.
3. Update the Rust manifest model and serde behavior so the implementation and JSON Schema describe
   the same contract.
4. Update `cli/golem-cli/src/versions.rs`:
   - change `sdk::MANIFEST` only when the YAML document version changes;
   - change `manifest_schema_version!()` only when using a new schema publication identifier.
5. If `sdk::MANIFEST` changes, replace old-version policy and upgrade behavior in
   `cli/golem-cli/src/app/manifest_version.rs` and `manifest_upgrade.rs` with the new current
   contract. Do not keep previous versions accepted or generate compatibility upgrades.
6. Check every schema-version consumer, especially:
   - `cli/golem-cli/src/lib.rs`
   - `cli/golem-cli/src/app/template/snippet.rs`
   - `cli/golem-cli/src/app/build/check/mod.rs`
   - tests containing embedded `$schema` references
7. Update all in-tree manifests, templates, examples, fixtures, tests, and generated artifacts that
   use the changed field or version.
8. Run focused validation, checks, and tests.

## Things To Watch

- Do not assume `sdk::MANIFEST` and `manifest_schema_version!()` should always match.
- When introducing a new field or enum value, ensure both serde parsing and JSON
  schema validation agree.
- If the CLI emits manifest templates/snippets, make sure they reference the new
  schema version.
- Historical schema directories are publication artifacts, not contracts that current CLI code
  must continue accepting. Leave unrelated history untouched, but do not update it or route current
  behavior through it.

## Useful Files

- `cli/golem-cli/src/versions.rs`
- `cli/golem-cli/src/lib.rs`
- `cli/golem-cli/src/app/template/snippet.rs`
- `cli/golem-cli/src/app/manifest_version.rs`
- `cli/golem-cli/src/app/manifest_upgrade.rs`
- `cli/golem-cli/src/app/build/check/mod.rs`
- `cli/golem-cli/src/model/app_raw.rs`
- `cli/schema.golem.cloud/app/golem/*/golem.schema.json`

## Verification Checklist

1. The current schema validates the changed manifest shape and rejects removed shapes.
2. Serde parsing and JSON Schema validation agree.
3. CLI version constants and emitted `$schema` references point to the sole current contract.
4. No compatibility parser, alias, old-version support, upgrade path, migration, or backfill was
   added or retained for the changed contract.
5. All in-tree manifests and generated examples use the current shape.
6. `cargo check -p golem-cli` passes.
7. Focused manifest schema/version tests pass with `--report-time`.
