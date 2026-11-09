---
name: golem-invoke-agent-kotlin
description: "Invoking a Kotlin Golem agent method from the CLI. Use when asked to call, invoke, or run a method on a deployed agent using golem agent invoke."
---

# Invoking a Golem Agent with `golem agent invoke`

Both `golem` and `golem-cli` can be used — all commands below work with either binary.

## Usage

```shell
golem agent invoke <AGENT_ID> <FUNCTION_NAME> [ARGUMENTS...]
```

This invokes a method on a deployed agent and **waits for the result**. The agent is automatically created on first invocation if it does not exist yet. Standard output, error, and log streams from the agent are streamed live to the terminal by default.

## Encoding Parameters

Pass one shell argument per user-supplied method parameter, in signature order. Do not pass a JSON object wrapping all parameters. A data-class-valued parameter is one argument.

Skip host-injected `Principal` fields and retain the order of the user-supplied inputs. Do not supply a principal value or placeholder.

| Method signature | CLI method and arguments |
|------------------|--------------------------|
| `fun prompt(text: String)` | `prompt '"hello"'` |
| `fun submit(request: Request)` with `data class Request(val text: String)` | `submit '{ text = "hello" }'` |
| `fun processOrder(orderId: String, quantity: Int)` | `processOrder '"order-456"' 42` |
| `fun history()` | `history` |

For a method with no user-supplied parameters, omit arguments; `history '{}'` supplies an extra argument. Quote each native literal as one shell argument. For a string parameter, if native literal parsing fails, the CLI uses the argument as raw text. Passing `'{"text": "hello"}'` to `prompt` therefore passes that entire JSON text as the string; it does not extract `text`.

Custom HTTP endpoints use a different rule: structured JSON bodies contain named body parameters, even for one parameter. Path/query/header parameters are not body fields; `UnstructuredText`/`UnstructuredBinary` parameters use raw bodies. Load `golem-http-params-kotlin` for HTTP mapping.

`--format json` selects output formatting, not input encoding.

## Output

Text output renders return values using Kotlin syntax. Multiple return values are rendered as a `Pair`/`Triple`, for example `Pair(1, "ok")`. A method returning `Unit` prints `void` in text mode.

For machine-readable output, use `--format json`, `--format yaml`, or `--format toon`. Streaming methods emit invocation lifecycle documents in order: `accepted`, `result`, any stream `item`/terminal events, and `finished`. Scalar fields accompanying streams are in the `result` event's `value`.

## Streaming Parameters and Results

Use `-` as the argument for exactly one direct stream parameter to bind it to stdin. By default, `--stdin-format value` reads one Kotlin value per line, strips only the line terminator, preserves blank lines as values, and does not accept multiline values. `--stdin-format raw` is available only for a direct `stream<binary>` or `stream<u8>` parameter. For `stream<binary>`, each logical item is a fixed 64 KiB chunk except the final shorter chunk. For `stream<u8>`, each byte is one logical item.

```shell
printf '1\nnull\n' | golem agent invoke 'MyAgent()' consumeValues -
cat input.bin | golem agent invoke 'MyAgent()' consumeBytes - --stdin-format raw
```

`--stdout-format value` is the default and renders stream items as Kotlin values. `--stdout-format raw` writes only bytes and requires exactly one direct `stream<binary>` or `stream<u8>` result.

```shell
golem agent invoke 'MyAgent()' produceValues --stdout-format value
golem agent invoke 'MyAgent()' produceBytes --stdout-format raw > output.bin
```

In structured CLI formats, invocation output is a sequence of lifecycle documents rather than one object or array. Streaming invocation is durable: disconnecting or pressing Ctrl-C detaches the current transport without cancelling the invocation. The CLI does not retry automatically. Use `--save-session <PATH>` on the initial invocation, then `--resume-session <PATH>` after a detach or `--takeover-session <PATH>` to fence an attached transport explicitly.

## Agent ID Format

The agent ID identifies the agent type and its constructor parameters:

```
AgentTypeName(param1, param2, ...)
```

The agent ID can optionally be prefixed with environment or application paths:

| Format | Description |
|--------|-------------|
| `AgentTypeName(params)` | Standalone agent name |
| `env/AgentTypeName(params)` | Environment-specific |
| `app/env/AgentTypeName(params)` | Application and environment-specific |
| `account/app/env/AgentTypeName(params)` | Account, application, and environment-specific |

For agents with no constructor parameters, use empty parentheses: `AgentTypeName()`. A phantom instance appends its UUID in brackets — see `golem-multi-instance-agent-kotlin`.

## Examples

```shell
golem agent invoke 'MyAgent()' getStatus
golem agent invoke 'MyAgent("user-123")' processOrder '"order-456"' 42
golem agent invoke 'ChatRoom("general")' sendMessage '"Hello, world!"'
golem agent invoke 'staging/MyAgent("user-123")' getStatus
```

## Available Options

| Option | Description |
|--------|-------------|
| `-t, --trigger` | Only trigger the invocation without waiting for the result (fire-and-forget) — see `golem-trigger-agent-kotlin` |
| `-i, --idempotency-key <KEY>` | Set a specific idempotency key; use `"-"` for auto-generated |
| `--no-stream` | Disable live streaming of agent stdout/stderr/log |
| `--stdin-format value\|raw` | Select stdin stream framing; defaults to `value` |
| `--stdout-format value\|raw` | Select stream result rendering; defaults to `value` |
| `--schedule-at <DATETIME>` | Schedule the invocation at a specific time (requires `--trigger`) — see `golem-schedule-agent-kotlin` |

## Idempotency

Every invocation uses an idempotency key. If not provided, one is generated automatically. The same idempotency key guarantees that the invocation is executed at most once, even if the CLI call is retried.

```shell
golem agent invoke -i my-unique-key 'MyAgent()' doWork
```

## Auto-Deploy

If the agent's component has not been deployed yet and the CLI is run from an application directory, `golem agent invoke` will automatically build and deploy the component before invoking.

## Value Syntax

The agent ID parameters and method arguments use **Kotlin syntax**, matching the CLI's `render`/`parse` rules for Kotlin (shared with the generated bridge clients):

- **A Kotlin `data class` is an anonymous record. Write it in braces, with `=` between field name and value: `{ fieldOne = 1, fieldTwo = "hello" }`.** Field names are `camelCase`. A colon (`{ fieldOne: 1 }`) is a parse error, and so is the class name in front of the braces.
- The named form `MyRecord(fieldOne = 1, fieldTwo = "hello")` works only for a record that the agent schema names, and the Kotlin generator never names one: a `data class` maps to a structural WIT `record<fieldOne:s32,fieldTwo:string>`, which keeps the class name for generated Kotlin code but not for the schema. Use the named form only when invoking an agent written in another language whose schema declares the type.
- Options are nullable: a bare value is `Some`, `null` is `None`. A nested option disambiguates `Some(None)` as `{ some = null }`.
- Enums/sealed classes: `MyEnum.CaseName` or `MyEnum.CaseName(value)` (the `MyEnum.` prefix is optional)
- Results: `Either.Right(value)` (= `Ok`) / `Either.Left(value)` (= `Err`); bare `Right(...)`/`Left(...)` are also accepted
- Lists: `listOf(a, b, c)` (a bracketed `[a, b, c]` form is also accepted)
- Tuples: `Pair(a, b)` / `Triple(a, b, c)`
- Flags: `MyFlags(readOnly = true, cacheable = false)`
- Rich types: `Text("...")`, `Binary("...")`, `Path("...")`, `Url("...")`, `Uuid("...")`, `Datetime("...")`, `Duration("...")`, `Quantity("...")`
- Maps use `{ k => v }`, not `mapOf(k to v)` (the generic parser does not accept Kotlin's own map literal)

```shell
golem agent invoke 'MyAgent("user-123")' updateProfile '{ displayName = "Alice", age = 30 }'
```
