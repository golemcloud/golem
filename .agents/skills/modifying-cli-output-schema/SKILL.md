---
name: modifying-cli-output-schema
description: "Adding or modifying Golem CLI structured output types, CliOutput implementations, command-output.schema.json, or DTO-backed output schema generators."
---

# Modifying CLI Output Schema

Use this skill when changing structured output from `golem-cli`, including:

- adding or modifying a `CliOutput` implementation;
- changing a DTO/view used by structured CLI output;
- editing `cli/golem-cli/command-output-schema/command-output.schema.json`;
- changing schema tests or arbitrary generators in `cli/golem-cli/src/model/cli_output.rs`.

This is different from the application manifest schema under
`cli/schema.golem.cloud/app/golem/`. For manifest schema version changes, use
`modifying-cli-manifest-schema` instead.

## Core Rules

1. Keep the public `$type` discriminator model. Every structured output document
   must have the right stable `$type` value.
2. `$type` values use suffixless command/action paths such as `agent.invoke`,
   `component.manifest-trace`, or `deployment.diff`. Do not add redundant
   `.result` or `.event` suffixes.
3. Keep machine-readable structured output on stdout and human logs, prompts,
   progress, and diagnostics on stderr.
4. Prefer typed schema definitions over `JsonValue`. Use generic JSON only when
   the payload is semantically arbitrary or cannot be related in JSON Schema
   draft-07.
5. Match serde's actual serialized shape, not the Rust type shape you expect.
   Tagged enums, flattened enum payloads, skipped fields, and custom serializers
   are common drift sources.
6. For exact object schemas, set `additionalProperties: false` and keep
   `required` entries aligned with `properties`.

## Important Files

- `cli/golem-cli/command-output-schema/command-output.schema.json` — public
  handwritten CLI output schema.
- `cli/golem-cli/src/model/cli_output.rs` — `CliOutput` registry checks,
  schema tests, and DTO-backed arbitrary generators.
- `cli/golem-cli/src/model/text/**` — many structured output view types.
- `cli/golem-cli/src/model/**` — CLI DTO/view models used by structured output.
- `Makefile.toml` — `check-cli-output-schema` and
  `update-cli-output-schema-summary` tasks.

## Generator Rules

Property-based schema examples should construct real Rust DTO/view values and
serialize them through `to_cli_output_value`.

Do not hand-build full output JSON documents in generators. Hand-built JSON is
acceptable only for:

- minimal negative schema tests;
- intentionally arbitrary `serde_json::Value` leaves;
- small helper payloads that are themselves semantically JSON values.

When schema coverage is expanded, expand the generator too. The generator should
exercise meaningful variants and nested shapes, not just the empty/default case.
If a generator reveals schema drift, inspect the serialized DTO and fix the
schema or the DTO intentionally.

## Accepted Generic JSON Leaves

These generic areas are intentional unless the task explicitly says otherwise:

- `ValueAndTypeJson.value`: draft-07 cannot validate it relationally against
  sibling `typ`.
- `AgentConfigEntryDto.value`: this is `NormalizedJsonValue` and is semantically
  arbitrary JSON.
- Manifest config JSON leaves inside typed manifest trace output.
- Raw/default/display secret values: valid shape depends on secret type and
  display context.

## Oplog Status

`agent.oplog` is the known remaining major gap.

Currently:

- `AgentOplogEntry` still keeps the public oplog payload generic in the CLI
  output schema.
- The output generator still uses an empty oplog wrapper rather than generating
  `PublicOplogEntry` variants.

Do not claim oplog payload typing is complete unless this has been implemented.
If oplog support is added later, update this skill in the same change to remove
this warning and document the new schema/generator workflow for public oplog
entries.

Useful implementation notes for future oplog work:

- Source types are the public oplog DTOs (`PublicOplogEntry` and nested public
  types) from `golem-common`.
- The generated OpenAPI in `openapi/golem-worker-service.yaml` already contains
  public oplog schema definitions that can be adapted carefully to the CLI
  draft-07 schema.
- Keep `ValueAndTypeJson.value` and JSON snapshot payload leaves generic unless
  custom relational validation is introduced.

## Workflow

1. Identify the affected output kind and `CliOutput` type.
2. Update the Rust DTO/view model if needed.
3. Update `command-output.schema.json` to match actual serde output.
4. Add or improve the generator in `cli_output.rs` using real DTO/view values.
5. Run focused schema tests and inspect failures as DTO/schema drift.
6. Check whether user-facing skills under `golem-skills/skills` need updates
   when CLI output field names, `$type` names, or examples change.
7. Check whether `golem-skills/tests` needs updates when output field names,
   `$type` names, JSON formatting, or invoke JSON unwrapping changes.
8. Regenerate the local output summary when the registry or output types change.

## User-Facing Skill Impact

CLI structured output changes can make embedded user-facing skills stale. Always
search `golem-skills/skills` when changing:

- machine-readable CLI field names, such as `resultJson` / `resultsJson`;
- `$type` naming conventions;
- examples showing `--format json`, `--format yaml`, or structured output;
- command names or flags used in skill instructions.

If any files under `golem-skills/skills` change, regenerate the generated How-To
Guide docs before finishing:

```shell
cargo make generate-docs-skills
```

CI rejects drift via `cargo make check-docs-skills`.

## Golem Skill Harness Impact

CLI structured output changes can break generated-application skill tests under
`golem-skills/tests`, especially the harness code that invokes `golem-cli` and
unwraps JSON output.

When public CLI output changes, inspect and update affected files under:

- `golem-skills/tests/harness/src/`;
- `golem-skills/tests/harness/tests/`;
- `golem-skills/tests/harness/scenarios/`.

The full scenario suite can require credentials, services, and significant time,
and is normally run later in PR/CI. Locally, run focused harness unit/build
checks only when harness TypeScript code or fixtures changed:

```shell
cd golem-skills/tests/harness
npm run build
npm test
```

## Validation

Run these for CLI output schema/generator changes:

```shell
cargo fmt --package golem-cli
cargo test -p golem-cli cli_output_schema_ --lib
cargo make check-cli-output-schema
cargo make update-cli-output-schema-summary
cargo check -p golem-cli
```

If arbitrary generators changed, rerun the generated-example prop test a few
times:

```shell
cargo test -p golem-cli cli_output_schema_accepts_registered_generated_examples --lib
```

Remove transient `cli/golem-cli/proptest-regressions/` files created by failing
local generator runs unless the project intentionally wants to commit that
regression seed.

## Common Drift Sources

- Nullable fields that are `Option<T>` in DTOs but schema forgot `null`.
- Enum case casing (`kebab-case`, `camelCase`, or Rust variant names).
- Internally tagged enum payloads that flatten fields into the same object.
- Custom serializers such as manifest trace `appliedLayers`.
- `skip_serializing_if` fields that should not be required.
- New nested DTO variants not covered by generators.

## Checklist

1. `$type` is stable and registered in both source and schema.
2. `$type` follows suffixless command/action path naming.
3. Schema matches actual `serde_json::to_value` output.
4. Output generator constructs real DTO/view values.
5. Important enum and nested variants are covered by examples or generators.
6. User-facing skills under `golem-skills/skills` have been checked when public
   output changed.
7. `golem-skills/tests` impact has been checked when public output changed.
8. Generated docs from user-facing skills were regenerated if `golem-skills/skills`
   changed.
9. Remaining `JsonValue` leaves are documented and intentional.
10. Focused schema tests, check task, summary update, and `cargo check -p
   golem-cli` pass.
