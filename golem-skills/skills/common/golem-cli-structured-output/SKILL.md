---
name: golem-cli-structured-output
description: "Understanding and parsing Golem CLI structured output. Use when using --format json, --format yaml, --format toon, discovering $type schemas with golem output-schema, or writing agents/tools that parse CLI output."
---

# Golem CLI Structured Output

Both `golem` and `golem-cli` can emit machine-readable structured output. Use this skill when building scripts, coding agents, test harnesses, or tools that parse CLI output.

## Formats

Use the global `--format` flag before the command:

```shell
golem --format json agent list
golem --format yaml component list
golem --format toon agent stream <AGENT_ID>
```

For automation, prefer `--format json` unless you specifically need YAML or TOON.

Human logs, prompts, progress, and diagnostics are written to stderr in structured formats. Parse stdout as the structured payload.

## Secret And Masking Behavior

Most secret-bearing structured outputs are masked by default. Pass the global `--show-secrets` flag only when you intentionally need the raw values.

`api-token.new` is an explicit exception: `golem api-token new` intentionally emits the newly generated token secret once, including in structured formats such as `--format json`, because the secret cannot be retrieved later. Store that output securely.

Plugin parameter values are masked by sensitive-looking parameter names. When a plugin parameter carries a secret, use a parameter name containing words such as `secret`, `token`, `password`, or `key` so CLI outputs mask it by default.

## `$type` Discriminator

Every structured output document has a top-level `$type` field. Branch on `$type` before reading command-specific fields.

Example:

```json
{
  "$type": "agent.list",
  "agents": [],
  "cursors": {}
}
```

`$type` values are stable output type identifiers. They often resemble CLI command/action paths, but they are not guaranteed to be literal command paths. Use schema metadata such as `x-golem-command` and, when present, `x-golem-commands` to understand which CLI command or commands can emit a type.

Naming examples:

- Top-level application commands use top-level names such as `new`, `templates`, `build`, `clean`, `generate-bridge`, and `deploy`.
- Entity commands usually use command-like names such as `agent.invoke`, `component.list`, or `environment.list`.
- Streaming outputs use the command family plus the streamed resource or event type, such as `agent.stream` for stream events and `agent.oplog` for oplog entries.
- Deploy subdocuments use `deploy.*` names, such as `deploy.diff` and `deploy.plan`, because they are emitted as part of `golem deploy`.
- Some semantic output types can be emitted by multiple commands; check `x-golem-commands` when present.

## Discover Output Schemas

`golem output-schema` is raw tooling output, not a normal structured CLI result. In text mode it prints compact single-line JSON. Use the global `--format` flag to request another raw schema format such as `pretty-json`, `yaml`, `pretty-yaml`, or `toon`.

List known output type names:

```shell
golem output-schema --types
```

Print a focused schema for one output type:

```shell
golem output-schema --type agent.invoke
```

Print a focused schema bundle for multiple output types:

```shell
golem output-schema --type agent.oplog --type agent.stream
```

Print the full raw schema only when broad cross-output context is needed:

```shell
golem output-schema
```

`--type` returns a pruned JSON Schema containing the selected output definitions and only the referenced definitions needed by those types. This is usually the best schema input for coding agents because it keeps context small.

## Single Documents, Streams, And Multi-Document Commands

Most structured commands emit exactly one document to stdout.

Some commands emit a finite set of structured documents during one command run. Different `$type`s may appear conditionally, and not every possible document appears every time. For example, `golem deploy` may emit `deploy.diff` and/or `deploy.plan`, followed by a final `deploy` success document.

Some commands emit a stream of documents:

| Command | `$type` | Shape |
|---------|---------|-------|
| `agent stream` | `agent.stream` | One document per stream event |
| `agent oplog` | `agent.oplog` | One document per oplog entry |

For compact `--format json`, each streamed document is emitted as one JSON line. Parse stdout line by line. For `pretty-json`, YAML, and TOON, parse stdout as a sequence or framed stream, not as one JSON object or array.

Structured stream output may be empty if there are no events or entries.

Focused schemas include `x-golem-output-mode` metadata:

| Mode | Meaning |
|------|---------|
| `single` | One structured document on success |
| `stream` | Multiple documents of the same event/entry type; parse until EOF or interruption |
| `multi-document` | Finite command-bounded output; multiple `$type`s may appear conditionally |

## TOON Frames

When using `--format toon`, each structured document is framed:

```text
@toon
<one TOON document>
@end
```

Parse stdout by splitting on exact `@toon` and `@end` marker lines.

## Text-Only Interactive Output

Some interactive terminal modes are text-only. For example, `agent list --refresh` continuously redraws the terminal and cannot be combined with structured formats such as `--format json`, `--format yaml`, or `--format toon`.

## Practical Parsing Guidance

- Use `golem output-schema --types` to discover possible `$type` values.
- Use `golem output-schema --type <TYPE>` to get a focused schema before parsing an unfamiliar output type.
- For single-document commands, parse stdout as one structured document.
- For multi-document commands, parse stdout as multiple documents and branch on `$type` for each one.
- For streaming commands, parse stdout as multiple documents and handle each document independently.
- Do not parse stderr as structured output; treat it as logs and diagnostics.
- Prefer `$type` and schema fields over text messages, table columns, or human wording.
