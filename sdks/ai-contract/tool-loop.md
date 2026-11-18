# The tool loop and the projection

**Layer:** binding: the toolsets, steps, tool errors, the projection, the run-start check and the
provider limits. Reference model: the `tool-limits` record.

## Toolsets

| Call form | Offered toolset |
| -- | -- |
| `run`, `stream` (bound form) | every tool declaration bound to the calling owner, once each, plus the local tools passed |
| `runWith`, `streamWith` (exact form) | only the local tools passed, or none |

No other tool reaches a model (Rule 6). `tools` in `llm-options` takes local tools only \[PROP\]. To
offer a bound tool's command in the exact form, code wraps it in a local tool that calls the tool's
client.

### The bound toolset

- **Sources.** Every tool declaration bound to the owner through a member of its `golem.tools`
  (`bash` by default) or a `Tool` field of its config, single or in a list. Bound memory database
  tools are `Tool` fields, so they are in the bound toolset (3.2); code that wants no database access
  uses the exact form.
- **Once each, by declaration.** A declaration reached through several sources is offered once. The
  library builds the toolset from guest tool reflection, `get-all-tools` as extended by 1.4 TC-09
  (C19, F22), which lists each reachable declaration once, `golem.tools` members included. The
  library does no deduplication and resolves no names of its own; the 6.2 ST-04 index stays on the
  host. Declaration identity is FT2's (GOL-1078). For each tool, the library needs from reflection:
  its `tool-target`, a raw-handler mark per command, its command paths, input and output schemas,
  declared errors, stream kinds and `mcp.name` override, in all six SDKs (Python after 8.1). TC-09
  blocks GOL-1359, GOL-1365 and GOL-1366 to GOL-1370 (F25).
- **Middleware.** A bound tool runs under the environment's and the declaration's middleware, never a
  field's own middleware (1.4 Rule 12, Rule 6). An agent with `bash` in both `golem.tools` and a
  `Tool` field offers one `bash`, without the field's middleware.
- **Commands.** Each command of a bound declaration becomes one function tool through the projection
  below. Raw-handler commands (2.2 HM11) are never offered.
- **Resolution time.** `golem.tools` is static wiring (1.4 Rule 20); its environment-wide layer comes
  from the Golem config cascade (1.4 F14). `Shared` `Tool` fields resolve when the call starts and
  stay pinned for the call.
- **Calls.** A bound tool called by the model is an ordinary tool invocation (Rule 7): it goes through
  2.1's invoke by declaration (FT16, GOL-1350) with the `tool-target` that reflection gave as its
  target (2.1 K25): `name(string)` for static wiring, `pinned(record { declaration, revision })` for
  a tool read from a `Shared<Tool>` field. FT16 ships the `name` case; the `pinned` case lands with
  whichever of FT16 and TC-09 lands second. The library passes the target through and never builds
  one itself. The call runs under the
  middleware above, with admission and execution authority as in 2.1 (the tool's own principal,
  FT12), and with caller identity kept (5.2: `original` stays, `immediate` is the calling agent or
  tool). The loop attaches no cards to bound tool calls (plan X9); to lend a card, code writes a local
  tool that calls the tool's or agent's client with `{ card }`.
- **Failures.** A bound tool's failure, denial or admission failure goes back to the model as a tool
  error and does not fail the run (Rule 7).
- **Idempotent commands.** Inside an idempotent command (2.1 FT6), bound tool calls run idempotent;
  promise and webhook waits fail with `idempotent-command-cannot-wait`; sleeps, provider back-off
  included, count toward the time bound; the first streamed output item ends rerun safety. A rerun
  before the first output repeats the provider call; it gets no derived idempotency key.

### Local tools

Local tools are defined in [interfaces.md](interfaces.md#local-tools). Their handlers run on the
calling invocation's task with the invocation's signal. A local tool that calls back into its own
serial agent, or forms A→B→A, gets 1.2's typed `reentrant-call` from the client; the handler decides
whether to return it to the model as a tool error.

## Steps

One step is one provider request.

1. The loop sends the messages so far, the system text and the offered tools.
2. If the answer has tool calls, the loop runs them one after another in the order the model returned
   them \[PROP\], appends one tool result per call, and starts the next step.
3. If the answer has no tool calls, the loop ends with that step's finish reason.

- **Step limit.** `stepLimit` \[PROP\] defaults to 10 \[PROP\]; a run can set its own. The last
  permitted step runs without tools \[PROP\], so the model must answer in text. When the loop reaches
  that step, the result's `finish` is `step-limit`. Reaching the limit is not an error.
- **Tool mode** \[PROP\]: `automatic` lets the model choose; `required` forces a tool call on the first
  step only, and later steps are automatic; `none` offers no tools and runs one step.
- **Structured output.** The value comes from the step that ends the loop ([operations.md](operations.md#options)).
- **Usage.** Each step reserves and commits its own usage ([usage-and-quota.md](usage-and-quota.md)).
- **Cancellation.** The signal goes to every provider request and every tool call; a cancelled step
  ends the run in the SDK's cancellation form.

## Run-start check

When a run starts, before anything is sent, the library rejects with `invalid-tools`, naming the tool
(Rule 8):

- two offered tools with the same model-facing name, after deduplication;
- a name or an input schema that some model the run may use cannot accept. Every member of a fallback
  composite is a model the run may use (`describe` lists them), so the check uses the strictest
  limits of all members;
- a bound command with an argument that projects to the reserved name `_stdin` \[PROP\];
- a wrapper tool (the agent→tool wrapper, GOL-1607) whose method has an argument that projects to
  `_agent` or `_agent_id`.

An argument named `idempotency_key` is not rejected. The projection never adds MCP's synthetic
`idempotency_key` argument, so the model never fills it (plan §4.1; 4.2 R2-5).

## The projection

Model-facing tool names, schemas and result envelopes follow the function-tool projection shared with
MCP (conventions, Errors 4), without MCP's `idempotency_key` argument (Rule 8). The projection is a
written contract with test vectors owned by 4.2 P3: [GOL-1596](https://linear.app/golem-cloud/issue/GOL-1596)
(the contract), [GOL-1598](https://linear.app/golem-cloud/issue/GOL-1598) (the MCP side) and
[GOL-1599](https://linear.app/golem-cloud/issue/GOL-1599) (the written clauses and the vectors). This
contract does not restate it; where the two differ, P3's written contract wins. The parts Golem AI
relies on:

- **Names**: `<tool>_<command path>` in real snake_case (camelCase and kebab-case words split, joined
  with `_`, lowercased), with the declaration's `mcp.name` in place of the tool name when it has one.
- **Input schemas**: the command's input record. A command's stdin projects to a `_stdin` string; a
  non-stdin input stream projects to a finite array. `json` projects to `{}` (4.3 A18). Capabilities
  (`Secret`, `Quota`) in an input are rejected, as in MCP.
- **Results**: the command's result value, never truncated. A declared error is the envelope
  `{ "error": <name>, "payload": <value> }`. An undeclared failure, a denial or an admission failure is
  a tool error with the same envelope and an error name from P3.
- **Wrapper tools**: one command per agent method, taking `_agent` (upsert by constructor parameters)
  or `_agent_id` (get by ID, never create), plus `create` (always a new agent; 4.1 Rule 13, plan X8).

Each SDK projects bound tools in the SDK from reflection, through its existing JSON Schema renderer
(Rust `reflection.rs` `to_json_schema`, TypeScript `schema/render.ts`, Effect `schemaRender.ts`,
Scala `SchemaRef.toJsonSchema`, Go `core/schema/jsonschema.go`, MoonBit
`reflection/json_schema.mbt`), with no host import (plan D10). Local tools go through the same
renderer from their SDK schemas.

The check is the reflection conformance corpus `test-data/reflection-conformance/v1.json`: P3's
vectors join it (GOL-1359 writes interim vectors, GOL-1599 replaces them), and every SDK's existing
corpus run then checks Golem AI's projection against the same expectations as MCP. Today's host
projection, which predates P3, is `golem-service-base/src/mcp/native_tool/` (it lowercases with `-`
→ `_`, reserves only `_stdin` and renders `{ "name", "payload" }`); GOL-1598 brings it to the
contract.

## Provider limits

Each provider adapter declares its limits on tool names and schemas \[PROP\]; the run-start check
applies them. The recordings confirm them (GOL-1355, GOL-1356).

> The types on this page use the contract's WIT-like notation ([README.md § Notation](README.md#notation)). It is notation only: Golem AI defines no WIT interfaces, and each SDK writes these types by hand in its own idiom. Unless the page's Layer line says otherwise, they are the reference model ([README.md § Layers](README.md#layers)): each SDK exposes their information in its own API design.

```text
record tool-limits {
  name-pattern: string,        // a regular expression the whole name must match
  name-max-length: u32,
  root-must-be-object: bool,
  max-tools: option<u32>,
}
```

| Provider | Name pattern | Max length | Notes |
| -- | -- | -- | -- |
| `anthropic` | `^[a-zA-Z0-9_-]+$` | 64 | `input_schema` must be an object schema |
| `bedrock` | `^[a-zA-Z0-9_-]+$` | 64 | Converse `toolSpec.inputSchema.json` |
| `openai`, `grok`, `openrouter` | `^[a-zA-Z0-9_-]+$` | 64 | OpenRouter forwards to the routed model, so the strictest common limit applies |
| `ollama` | `^[a-zA-Z0-9_-]+$` | 64 | No documented limit; the common limit is used so a run that may fall back to another provider is checked the same way |

Every built-in provider requires an object at the root of an input schema, which the projection
always produces.
