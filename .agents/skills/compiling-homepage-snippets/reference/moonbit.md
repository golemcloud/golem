# MoonBit home-page snippets: how they are checked and compiled

Each home-page feature has its own short MoonBit snippet, displayed as is:
`website/snippets/<chapter>/<feature>/moonbit/<display-filename>.mbt`.
A line ending in ` //@` is highlighted on the page while its snippet shows;
the page strips the marker and it never counts toward the width limit. The
harness strips the markers (`sed -E 's,[ ]+(//|#)@$,,'`), writes the stripped
text into its own executable Moon package next to the code the snippet omits
(same line numbers as the display file), runs the Golem MoonBit code
generators, builds every package for wasm with the in-tree SDK and links each
module into a Golem component.

Everything is reproducible from the repository root
(`<repo>`):

```sh
bash .amp/in/snippets/moonbit/check.sh   # static checks, codegen, build, link; ends with ALL_OK
bash .amp/in/snippets/moonbit/neg.sh     # 16 negative controls + a clean run; ends with NEG_ALL_OK
```

A warm `check.sh` run takes about 15 s (32 code-generator runs, one
`moon build`, 16 component links); `neg.sh` runs `check.sh` 17 times, about
8 minutes. The first run is longer while Moon compiles the SDK and the
generator.

## Display files

| Display file | Harness package |
|---|---|
| `agents/overview/moonbit/triage_agent.mbt` | `harness/agents_overview` |
| `agents/models/moonbit/draft_agent.mbt` | `harness/agents_models` |
| `agents/team/moonbit/planner_agent.mbt` | `harness/agents_team` |
| `agents/waits/moonbit/checkout_agent.mbt` | `harness/agents_waits` |
| `agents/stream/moonbit/writer_agent.mbt` | `harness/agents_stream` |
| `agents/readonly/moonbit/stock_agent.mbt` | `harness/agents_readonly` |
| `tools/overview/moonbit/digest_agent.mbt` | `harness/tools_overview` |
| `tools/custom/moonbit/build_agent.mbt` | `harness/tools_custom` |
| `tools/requests/moonbit/stripe_refund.mbt` | `harness/tools_requests` |
| `tools/permissions/moonbit/orders_access.mbt` | `harness/tools_permissions` |
| `tools/secrets/moonbit/support_agent.mbt` | `harness/tools_secrets` |
| `tools/publish/moonbit/account_agent.mbt` | `harness/tools_publish` |
| `artifacts/overview/moonbit/brief_agent.mbt` | `harness/artifacts_overview` |
| `artifacts/routes/moonbit/board_agent.mbt` | `harness/artifacts_routes` |
| `artifacts/routers/moonbit/exports_router.mbt` | `harness/artifacts_routers` |
| `artifacts/login/moonbit/payslip_agent.mbt` | `harness/artifacts_login` |

There is no `agents/sqlite/moonbit` (owner's decision: MoonBit has no
embedded SQLite; the page shows a note). `check.sh` fails if that folder
appears. So the page's 17 features have 16 MoonBit display files.
`tools/publish` is a code snippet (`account_agent.mbt`); the shared
`golem.yaml` it replaced is gone and nothing checks it any more.

The display files omit what MoonBit keeps outside source files (package
imports live in `moon.pkg`) and the `///|` block separators `moon fmt` would
add. They are hand-formatted with 2-space indentation and are not
`moon fmt`-canonical; do not run `moon fmt` on them.

## Prerequisites and versions

- **MoonBit toolchain**: `moon 0.1.20260827 (d0aaa07)`, `moonc
  v0.10.11+6ff76a5f9 (2026-08-28)`, installed into a private `MOON_HOME` at
  `.amp/in/langs/moonbit/moonhome` with the standard installer
  (`curl -fsSL https://cli.moonbitlang.com/install/unix.sh | MOON_HOME=<dir> bash`).
  A recent toolchain is required: the SDK and harness use the `moon.mod` /
  `moon.pkg` DSL (feature flags `rr_moon_mod,rr_moon_pkg`); the older
  `~/.moon` (`moon 0.1.20260427`) fails with "Unexpected key 'readme' found
  in moon.mod". `check.sh` uses `$MOON_HOME` if set, otherwise
  `.amp/in/langs/moonbit/moonhome` if it exists, prepends `$MOON_HOME/bin` to
  `PATH`, and prints which `moon` it runs.
- **SDK WIT bindings**: `sdks/moonbit/golem_sdk/{interface,gen,world,async-core}`
  are generated and git-ignored; they must exist. If missing, install the
  Golem wit-bindgen fork and regenerate (as `sdks/moonbit/AGENTS.md` says):
  ```sh
  cd sdks/moonbit/golem_sdk
  ./scripts/install-wit-bindgen.sh   # Golem fork, branch wit-bindgen-golem-1.6 (wit-bindgen-cli 0.59.0)
  ./scripts/regen-bindings.sh
  ```
  In this checkout the fork binary is at
  `.amp/in/langs/moonbit/tools/bin/wit-bindgen`; the bindings were already
  present.
- **wasm-tools** `1.247.0` (`~/.cargo/bin/wasm-tools`, optional): if on
  `PATH`, every module is also linked into a component with the SDK WIT and
  validated.
- No network access once the toolchain and bindings exist; core mooncakes
  come with the toolchain and every other module resolves through the Moon
  workspace.
- Nothing outside `.amp/in/snippets/moonbit/` is written except build
  caches (`harness/_build`, the SDK's `_build` dirs).

## Harness layout

```
.amp/in/snippets/moonbit/
  check.sh  neg.sh  PROCESS.md
  harness/                        Moon module golemcloud/home_snippets_v3
    moon.mod                      imports golem_sdk@0.5.1, the invented libs and the stub clients
    moon.work                     workspace: ., ../libs/*, ../stubs/*, sdks/moonbit/golem_sdk{,_tools}
    util/                         library package: HTTP request builders, read_text, in_days
    <chapter>_<feature>/          one executable package (= one Golem component) per snippet
      moon.pkg.in                 imports for the snippet + harness (copied to moon.pkg each run)
      harness.mbt                 fn main {} and the code the snippet omits
      snippet_<file>.mbt          marker-stripped copy of the display file (written by check.sh)
      golem_*.mbt, moon.pkg       generated by check.sh (do not edit)
  libs/                           invented libraries, each its own Moon module
    golem_ai/       moon.mod llm/moon.pkg llm/llm.mbt   golemcloud/golem_ai (package llm)
    golem_secrets/  moon.mod moon.pkg secrets.mbt       golemcloud/golem_secrets
  stubs/                          generated-client stand-ins (moon.mod.json, like `golem build` output)
    research-agent-guest-client/  package client (@research): ResearchAgentClient, Facts
    slack-agent-guest-client/     package client (@slack): SlackAgentClient
    bash-tool-guest-client/       package client (@bash): BashClient, BashResult, BashError
    crm-find-customer-tool-guest-client/
                                  package client (@crm): CrmFindCustomerClient (MCP-imported tool)
```

One package per snippet keeps the snippets from colliding (several define
`RefundResult`, `Refund` tools or agents with clashing generated clients)
and makes each negative control hit exactly one package.

What each `harness.mbt` supplies:

| Package | Harness code |
|---|---|
| `agents_overview` | `classify(body) -> Label` (stand-in classifier, already typed: TS `Label.parse(await classify(...))`), `apply_label(inbox, id, label)` (JSON POST that fails on non-2xx); `Email` and `Label` are displayed |
| `agents_models` | nothing (`Review` and `Draft` are displayed) |
| `agents_team` | `Plan { facts : @research.Facts, agents, tools }` (`#derive.golem_schema`) |
| `agents_waits` | `PaymentEvent`, a real `#derive.agent struct MailerAgent` with `remind` (so the generated `MailerAgentClient` exists), `create_payment_link`, `send_survey`, `in_days` |
| `tools_overview` | `summarize_text(page)` (stand-in for TS `summarizeText`); `get(url)` and `read_text` from `util` |
| `tools_requests` | the refund tool of the Secrets/old Tools snippet, calling the displayed `stripe_refund`; `form_post`, `read_text` |
| `tools_permissions` | `#derive.agent struct OrdersAccess` whose `share(agent)` / `revoke()` call the displayed `share_orders` / `revoke_orders` (target id from `@api.resolve_agent_id`) |
| `tools_custom` | nothing (bash client from `stubs/`) |
| `tools_publish` | nothing (the MCP tool client stand-in is in `stubs/crm-find-customer-tool-guest-client`) |
| `artifacts_routers` | a real durable `#derive.agent struct ExportAgent` (`new(id)`, `csv() -> @schema.AgentStream[Array[Byte]]`), so the generator emits the same-component client `ExportAgentClient`; `is_word(id)` (the `[\w-]+` rule of the TS regex) |
| `tools_secrets` | a `refund` tool (same signature as the Requests harness, trivial body) and `RefundResult`, so the generator emits the typed tool client `RefundClient` in this component |
| `agents_stream`, `agents_readonly`, `artifacts_overview`, `artifacts_routes`, `artifacts_login` | nothing |

`util` (`golemcloud/home_snippets_v3/util`): `json_post`, `form_post`
(form-encoded HTTPS POST with percent-encoding and extra headers, the
`URLSearchParams` equivalent), `get` (a body-less HTTPS GET, the `fetch(url)`
equivalent), `send_ok`, `read_text` (all on the SDK's P3
`@http`), and `in_days(n)` (`@systemClock.Instant` now + n × 86 400 s).
Harness packages bring the names a snippet calls unqualified into scope
with `using @util {...}`.

## How display files are spliced (check.sh)

1. **Discover**: `find website/snippets -path '*/moonbit/*' -type f`; the
   package is `<chapter>_<feature>`. A display file without a package, or a
   package without a display file, fails.
2. **Static checks** on the display text with the trailing ` //@` / ` #@`
   markers stripped: ≤ 31 lines, every line ≤ 68 characters, no tabs, no
   marker left anywhere but at a line end; every line whose code part
   (string literals removed) contains `//` must be a comment line
   (`^\s*//`) ending with `:` (a highlighted comment line ends in `: //@`).
   `///` doc comments are exempt: in MoonBit a doc comment *is* the method
   description (the generator copies it into the agent type), the
   counterpart of TS `description: '...'`, and is not a sentence ending in
   a colon (`tools/publish`).
   Prints `<file>: N lines, longest M, markers K` for each.
3. **Splice**: `moon.pkg.in` → `moon.pkg` (the generators rewrite
   `moon.pkg`), delete old `golem_*.mbt` / `snippet_*.mbt`, then write the
   marker-stripped display text to `<package>/snippet_<file>.mbt`. Stripping
   keeps every line, so compiler errors point at the displayed line
   numbers.
4. **Codegen** (from `sdks/moonbit/golem_sdk_tools`), per package `p`:
   ```sh
   moon run cmd -- reexports <repo>/sdks/moonbit/golem_sdk <harness>/<p>
   moon run cmd -- agents <harness> --component-dir <p>
   ```
   This emits `golem_reexports.mbt`, `golem_exports.mbt`, `golem_agents.mbt`,
   `golem_derive.mbt`, `golem_clients.mbt` (+ `golem_tools.mbt`,
   `golem_tool_clients.mbt` for tools), adds the generated imports
   (`@schema`, `@common`, `@types`, `@rpc`, `@permissions`, `@agentHost`,
   `@systemClock`, …) and the wasm export list to `moon.pkg`. The generator
   validates the Golem annotations (mount paths and variables, routers,
   file bindings, read-only, tools) and rejects invalid ones.
5. **Build**: old `<p>.wasm` files are deleted, then
   `cd <harness> && moon build --target wasm` (a real wasm build, stronger
   than `moon check`). Outputs
   `_build/wasm/debug/build/golemcloud/home_snippets_v3/<p>/<p>.wasm`.
6. **Link** (if `wasm-tools` exists), per package:
   ```sh
   wasm-tools component embed <repo>/sdks/moonbit/golem_sdk/wit <p>.wasm --encoding utf16 -o _build/<p>.embed.wasm
   wasm-tools component new _build/<p>.embed.wasm -o _build/<p>.component.wasm
   wasm-tools validate --features all _build/<p>.component.wasm
   ```
7. Every command is printed (`<repo>` for the repository root) with its exit
   code; `ALL_OK` is printed only if everything passed.

## Negative controls (neg.sh)

`neg.sh` reruns `check.sh` with `MUTATE_FILE=<display file>` and
`MUTATE_SED=<sed -E expr>`; `check.sh` then applies the expression to the
marker-stripped text of that one file (the display file is never modified). Each
control must make `check.sh` fail with an error located in that snippet's
package and matching the expected text; every display file must have a
control; a final clean run must print `ALL_OK`.

| Snippet | Planted wrong Golem API use | Rejected by |
|---|---|---|
| triage_agent | `#derive.golem_schema` → `#derive.golem_scheme` | moonc: `Email does not implement trait WireSchema` (generated dispatch) |
| draft_agent | `fn DraftAgent::new(` → `fn DraftAgent::create(` | golem_sdk_tools: `AgentParseError` (agents need `new`) |
| planner_agent | `.trigger_post(` → `.post.trigger(` | moonc: `SlackAgentClient has no field post` |
| checkout_agent | `schedule_cancelable_remind(` → `schedule_remind(` | moonc: `Unit has no method cancel` |
| writer_agent | `out.write_one(reply)` → `out.write_all(reply)` | moonc: type mismatch |
| stock_agent | `#derive.agent` → `#derive.agent("ephemeral")` | golem_sdk_tools: `AgentParseError` (read-only methods need a durable agent, `lib/agents.mbt:1839`; the same file with `#derive.read_only` also removed passes) |
| brief_agent | `expose_files("/", …)` → `expose_files("/*", …)` | golem_sdk_tools: `AgentParseError` (a `/*` subtree needs a `/$1` target) |
| board_agent | `endpoint(post="/cards")` → `post="/cards/{id}"` | golem_sdk_tools: `AgentParseError` (path variable is not a parameter) |
| exports_router | `#derive.http_handler` → `#derive.endpoint(get="/")` | golem_sdk_tools: `AgentParseError` (router methods must be HTTP handlers) |
| payslip_agent | `caller : @common.Principal` → `caller : String` | moonc: `Oidc is unbound` |
| digest_agent | `SummarizeClient::new().summarize(` → `Summarize::client().summarize(` | moonc: `Summarize has no method client` |
| build_agent | `OutlineClient::new().outline(` → `Outline::client().outline(` | moonc: `Outline has no method client` |
| stripe_refund | drop `.unwrap()` after `@http.send(...)` | moonc: type mismatch (`Result` vs `@http.Response`) |
| orders_access | `lifespan=@permissions.Lifespan::iso8601("PT1H")` → `lifespan="PT1H"` | moonc: type mismatch |
| account_agent | drop `.unwrap()` after `.crm_find_customer(email)` | moonc: `Result[@tool.TypedToolInvocation[…], @tool.ToolError[…]] has no method collect` |
| support_agent (secrets) | pass `self.config.stripe_key` (a `@config.Secret[String]`) straight to `RefundClient::refund` | moonc: type mismatch (`@config.Secret[String]` vs `@secrets.SecretHandle[String]`) |

The code generator prints only the error type (`AgentParseError`), not its
message, so those five controls are identified by the failing
`agents <package>` step.

## Real SDK APIs used by the snippets

- Agents: `#derive.agent`, `fn T::new(...)` constructors (their parameters
  are the agent's identity), `pub fn` / `pub async fn` methods;
  `#derive.golem_schema` records and enums; `Array[Email]` parameters and a
  `Map[String, Label]` result; `#derive.mount("/checkout/{order_id}")`;
  `@webhook.create`, `WebhookHandler::url`, `wait`,
  `WebhookRequestPayload::text`; generated same-component client
  `MailerAgentClient::get(id).schedule_cancelable_remind(at)` →
  `@agentHost.CancellationToken::cancel`; `@monotonicClock.wait_for(ns)`
  (WASI P3 clock, made durable by the worker executor,
  `golem-worker-executor/src/durable_host/p3/clocks.rs`);
  `@reflection.get_all_agent_types` / `AgentType::name` / `methods` /
  `AgentMethod::name`, `@reflection.get_all_tool_types` /
  `ToolType::lookup_name`; guest-client shapes `get`, awaited call,
  `trigger_<m>`; `#derive.read_only` on a plain method (generated
  `read_only: Some({ cache_policy: UntilWrite })`).
- Tools: `#derive.tool("summarize")` with an `async fn` returning a plain
  `String` (no error enum), generated `SummarizeClient::new().summarize(url)`
  → `Result[String, @tool.ToolError[@tool.NoToolError]]`;
  `#derive.tool("outline", requires_filesystem=true)` (generated
  `requires_filesystem: true`), generated `OutlineClient::new().outline(file)`;
  `@http.send`; `@permissions.PatternGrant`, `Bounds::new(lower_positive=…)`,
  `derive_from_wallet`, `PersistentCard::derive_child(bounds,
  lifespan=Lifespan::iso8601("PT1H"))` (`derive` is a MoonBit keyword),
  `install(Agent(id))`, `revoke()` (returns the revoked count, ignored).
  `share_orders` / `revoke_orders` are `raise` functions, the MoonBit form of
  TS `.unwrap()`.
- MCP (`tools/publish`): a `///` doc comment on an agent method becomes the
  method's `description` in the generated agent type (`golem_agents.mbt`:
  `description: "Summarize a customer account by email"`), which an MCP
  deployment exposes as the tool description. The imported tool is called
  through the generated tool client shape of
  `cli/golem-cli/src/bridge_gen/moonbit/tool.rs`: `CrmFindCustomerClient::new()`,
  a non-async leaf method `crm_find_customer(email)` (the projection declares
  stdout, so it starts an invocation) returning
  `Result[@tool.TypedToolInvocation[R, E], @tool.ToolError[E]]`, then the
  real SDK `TypedToolInvocation::collect()` →
  `@tool.CollectedToolInvocation { result, stdout, stderr }`
  (`golem_sdk/tool/client.mbt:40,234`); `result.structured` is typed by the
  upstream output schema (`golem-mcp-import/src/tool.rs:160-168`, checked for
  MoonBit by `cli/golem-cli/tests/bridge_gen/mcp_import.rs:204-250`).
- Secrets: `#derive.config struct SupportConfig { stripe_key :
  @config.Secret[String] }`, constructor parameter
  `config : @config.Config[SupportConfig]` (stored as `config.value`; the
  generator emits the config declaration and `load_field` for the secret
  key), generated typed tool client `RefundClient::new().refund(charge,
  amount, api_key)` → `Result[RefundResult, @tool.ToolError[RefundError]]`,
  `.unwrap()` as in `tools/custom`.
- Artifacts: `#derive.expose_files(route, path)` (agent file bindings,
  generated as `FileMapping::Exact { public_path, file_path }`), an `async
  fn T::new(...)` constructor that writes files when the agent is created
  (as `FileOwner` in `golem_sdk_example1`), `#derive.http_router` +
  `#derive.mount("/exports")` on an empty struct with one
  `#derive.http_handler pub async fn serve(_self, request :
  @http.HttpRequest) -> @http.HttpResponse` (the MoonBit raw handler, the
  form of `defineHttpRouter(...).implementRaw(...)`; generated as an
  ephemeral agent with an `Any` endpoint), `@http.HttpRequest` `method` /
  `path` (the original public path), `@http.HttpResponse { status, headers :
  Array[@http.HttpHeader { name, value : Array[Byte] }], body :
  @schema.AgentStream[Array[Byte]] }` built as plain records; an empty body
  `AgentStream::produce(async fn(_) { })`; a generated same-component client
  `ExportAgentClient::get(id).csv()` returning
  `@schema.AgentStream[Array[Byte]]`, passed straight on as the response
  body (`AgentStream`'s `IntoWire` forwards the received stream handle,
  `golem_sdk/schema/agent_stream.mbt:349`), `#derive.endpoint(post=…/get=…)`,
  `#derive.mount_auth(true)` (`auth_details: Some({ required: true })`),
  `caller : @common.Principal` parameter (host-injected, matched on
  `Oidc({ sub, .. })`), an optional unstructured binary result
  `@schema.UnstructuredBinary[Pdf]?` with a phantom `Pdf :
  @schema.AllowedMimeTypes` (`["application/pdf"]`, the MoonBit form of
  `s.unstructuredBinary({ mimeTypes })`; `None` is a 404, as
  `WeatherAgent::get_report` in `golem_sdk_example1`), `Inline(bytes~,
  mime_type=…)`, `@schema.AgentStream[String]` built with
  `AgentStream::produce` / `write_one`, `@fs.get_root_dir`,
  `@fs.read_string`, `@fs.read_bytes`, `@fs.write_string`,
  `Descriptor::create_directory_at`.

## Invented libraries

| Import path (alias) | Used as | Why | Harness implementation |
|---|---|---|---|
| `golemcloud/golem_ai/llm` (`@llm`) | `@llm.openai(name)`, `@llm.anthropic(name)`, `@llm.generate_text(model=, prompt=)`, `@llm.generate_object(model=, prompt=) -> T`, `@llm.chat_object(messages, model=) -> T`, `@llm.Message` | No MoonBit LLM library exists (the moonbit LLM skill says to hand-roll HTTP). Golem publishes the provider-neutral `golem:llm` interface and the Rust crates `golem-ai-llm`, `-openai`, `-anthropic`; this is their plausible MoonBit binding with the Vercel AI SDK surface the TS snippets use (`openai(...)`, `anthropic(...)`, `generateText`, `Output.object`) and OpenAI's `chat.completions.parse`. Structured output is typed by any `T : @schema.IntoSchema + FromJson`, i.e. a `#derive.golem_schema` record. Reworked from the previous harness's `send`/`json_schema`. | Type-correct; every call raises `LlmError` at runtime. |
| `golemcloud/golem_secrets` (`@secrets`) | tool positional `api_key : @secrets.SecretHandle[String]`, `api_key.reveal()`; **shim** `@secrets.SecretHandle::from_config(self.config.stripe_key)` in the Secrets snippet | The SDK has `@config.Secret[T]` (a config key) and the untyped `@model.GuestSecretHandle`, but no typed transferable handle usable as a tool argument (TS has `s.secret(z.string())` + `golem:secrets/reveal`). Unchanged from the previous harness. | Real implementation over the in-tree SDK: wraps `@model.GuestSecretHandle`, implements `IntoSchema/FromSchema/WireSchema/IntoWire/FromWire` as a `secret<T>` node, `reveal()` via raw `golem:secrets/reveal`, `from_config` via `golem:agent/host.get-config-value`. |

## Stubs (generated clients for other components)

`golem build` generates a guest-client module per dependency
(`golem-temp/bridge-sdk/moonbit/internal/<name>-guest-client`, package
`client`, legacy `moon.mod.json`). The harness ships stand-ins with the same
module names, package and type names:

| Module (alias) | Source | Behaviour |
|---|---|---|
| `research-agent-guest-client/client` (`@research`) | Output of the in-tree `golem_sdk_tools agents` for an agent `ResearchAgent(topic)` with `gather() -> Facts` and `refresh()`, plus the generated `golem_derive` impls for `Facts { summary, sources }` (type id `research.Facts`) | Real Golem RPC through `@rpc.AgentClient` (`get`, `drop`, `gather`, `trigger_*`, `schedule_*`, `schedule_cancelable_*`). The real bridge for a Rust agent comes from `cli/golem-cli/src/bridge_gen/moonbit` with the same signatures. |
| `slack-agent-guest-client/client` (`@slack`) | Same generator, agent `SlackAgent(channel)` with `post(text)` | Real RPC, as above. |
| `bash-tool-guest-client/client` (`@bash`) | Hand-written after `bridge_gen/moonbit/tool.rs` and the bash 0.2.x schema (`cli/golem-cli/src/command_handler/ssh/bash-0.2.0.json`): `BashClient::new()`, `new_for`, `drop`, `async run(script, cwd, timeout : UInt) -> Result[BashResult, @tool.ToolError[BashError]]`, `BashResult { stdout, stderr, exit_code : Byte, cwd }` | `run` aborts at runtime. |
| `crm-find-customer-tool-guest-client/client` (`@crm`) | **Stand-in, labelled as such in the file.** Hand-written after `bridge_gen/moonbit/tool.rs` for the MCP import `{ name: crm, prefix: crm }` whose tool `find_customer` has input `{ email: string }` (`additionalProperties: false`, so no extra-properties map parameter) and output schema `{ name: string, plan: string }`: `pub(all) struct CrmFindCustomerClient { client : @tool.ToolClient, inherited }`, `new()`, `drop()`, `crm_find_customer(email) -> Result[@tool.TypedToolInvocation[CrmFindCustomerResult, CrmFindCustomerError], @tool.ToolError[CrmFindCustomerError]]`, `CrmFindCustomerResult { structured : CrmFindCustomerStructured { name, plan }, content }`, `CrmFindCustomerError::McpToolError(String)`. The record type names and the `content` type are placeholders (the snippet names neither); `collect()` and `TypedToolInvocation` are the real SDK types. | `crm_find_customer` aborts at runtime. |

To regenerate the two agent stubs: put `#derive.agent` structs with those
constructors and methods (and `#derive.golem_schema pub(all) struct Facts`)
in a scratch executable package of the harness, run the two codegen commands
above on it, and copy the client blocks from `golem_clients.mbt` and the
`Facts` impls from `golem_derive.mbt` into `stubs/*/client/client.mbt` (with
`moon.pkg` importing `@agents @schema @model @common @types @rpc
@permissions @agentHost @systemClock`).

## SDK gap: forwarding a config secret to a tool (Secrets snippet)

TS passes `this.config.stripeKey` straight as the tool's `apiKey`. The
MoonBit SDK cannot express that; the display file therefore converts the
config secret explicitly with the labelled shim
`@secrets.SecretHandle::from_config(...)` (libs/golem_secrets). Found by
reading the source and by compiling (passing the config value directly, or
declaring the tool parameter as `@config.Secret[String]`, fails):

- `sdks/moonbit/golem_sdk/config/top.mbt:16` `pub(all) struct Secret[T] {
  key : Array[String] }` carries only the config key and implements only
  `ConfigField`/`WireConfigField` (`config/wire.mbt:133`). It has no
  `IntoSchema`, `WireSchema`, `IntoWire` or `FromWire`, so it cannot be a
  tool argument: the generated `golem_tool_clients.mbt` fails with
  "`@config.Secret[String]` does not implement trait `@schema.IntoWire`"
  (and `WireSchema`/`IntoSchema`/`FromWire` in `golem_tools.mbt`).
- There is no typed secret type for tool parameters at all: the only
  carrier is the untyped `@model.GuestSecretHandle`
  (`golem_sdk/schema_model/secret_handle.mbt:12`), which has
  `IntoWire`/`FromWire` (`schema/wire.mbt:782-804`) but no `WireSchema` /
  `IntoSchema` (no inner payload type). The SDK's own test component
  hand-rolls a typed wrapper (`test-components/tool-streaming-moonbit/component/matrix_resource.mbt:1-120`).
- `Secret::with_handle` (`config/top.mbt:102-117`) lends the raw
  `@types.Secret` and then `defer raw_handle.drop()` (line 115); an owned
  handle moved into a tool call would be dropped again, so it cannot be
  used to forward the secret either. `Secret::load_handle` (line 64), which
  returns the owned handle, is private.
- Unlike TypeScript there is no unsafe adoption: the MoonBit types reject
  the wrong value at compile time; the gap is a missing feature.
- Fix: add a typed transferable `Secret` handle type to the SDK (what
  `@secrets.SecretHandle[T]` implements: a `secret<T>` schema node over
  `GuestSecretHandle`, plus `reveal()`), let the tool generator accept it
  for tool parameters, and give `@config.Secret[T]` a public
  `handle() -> SecretHandle[T]` (or make `load_handle` public and typed), or
  implement `IntoSchema`/`WireSchema`/`IntoWire` for `@config.Secret[T]`
  so lowering loads a fresh owned handle from the config key.

## Tools gap: `lexmatch` with captures is rejected by the code generator (Routers snippet)

- What fails: a component whose source contains the current `lexmatch`
  capture syntax, e.g.
  `lexmatch path { re"^/exports/" + (re"[\w\-]+" as id) + re"\.csv$" => … }`,
  fails both `golem_sdk_tools` commands (`reexports` and `agents`) with
  `AgentParseError` ("Invalid source in …: Unexpected token `+`, you may
  expect `=>`").
- Where: `sdks/moonbit/golem_sdk_tools/moon.mod:8` pins
  `moonbitlang/parser@0.3.18`; `lib/agents.mbt:1456-1462` (and the same
  check in `lib/tools.mbt:1474`, `lib/component_role.mbt:145`) turns any
  parser report into `AgentParseError`. Parser 0.3.18 accepts only the old
  form `(re"…" (re"…" as id) re"…")`, which moonc `v0.10.11` rejects
  ("unexpected token `(`"); moonc accepts only the `+` form, and the
  `after=` binding form is likewise rejected by the parser. No capture form
  is accepted by both.
- How we know: compiled the snippet with the `lexmatch` form (moonc
  accepted it, the rest of the component built and linked once the regex
  match was replaced); parsed the same text with a scratch program calling
  `@parser.parse_string` from `moonbitlang/parser@0.3.18`, which printed
  the report above.
- Fix: upgrade `moonbitlang/parser` (and `moonbitlang/lexer`) in
  `golem_sdk_tools` to a release that parses the current `lexmatch`
  syntax (not verified which: the local mooncakes index has no newer
  version), or have the generator skip function bodies it cannot parse.
  Workaround used here: string patterns.

## Faithfulness notes

- Highlights: each TS ` //@` line maps to the line carrying the same Golem
  call or declaration (`#derive.*` attributes for TS options, the
  `pub fn`/`fn` header for method declarations, `) -> @schema.AgentStream`
  for `returns: s.stream(...)`, `wait()` lines for `await webhook`). A TS
  line that MoonBit splits in two to stay within 68 characters is marked on
  both lines (`agents/team`: `.gather()` and `.map(t => t.lookup_name())`).
  In `tools/custom`, the two TS lines `toolDefinition('outline', {` and
  `requiresFilesystem: true` map to one `#derive.tool(...)` line, marked
  once.
- Clients (`agents/team`, `agents/waits`, `tools/custom`, `tools/overview`,
  `tools/publish`, `artifacts/routers`) are used inline without `drop()`,
  mirroring the TS one-liners.
- `agents/overview`: TS `id: { inbox }` is the constructor parameter, so
  the identity comment and highlight are on `fn TriageAgent::new(inbox :
  String)`. `z.record(z.string(), Label)` is `Map[String, Label]`.
  `Label.parse(await classify(...))` is `classify(...)` returning the typed
  `Label` (the harness stand-in), so there is no separate parse.
- `agents/models`: the TS snippet shows three `import` lines; MoonBit source
  has no imports (they live in `moon.pkg`), so the library is visible only
  through the `@llm.` prefix.
- `agents/team`: the comment keeps "a Rust ResearchAgent" (the call stays
  cross-language). The cross-component clients are the generated
  guest-client packages `@research` / `@slack`; TS `.post.trigger(...)` is
  the generated `trigger_post(...)`. Method names come from
  `AgentType::methods()` / `AgentMethod::name()`.
- `agents/waits`: TS `setTimeout` sleep → `@monotonicClock.wait_for` in
  nanoseconds; the cancelable schedule is `schedule_cancelable_remind`.
- `agents/readonly`: TS `init: () => ({ units: 0 })` is `units: 0` in the
  constructor; `z.int()` is `Int`.
- `tools/overview`: TS `fetch(url)` + `.text()` is `@http.send(get(url))`
  + `read_text` (harness builders, `util`); `ok(...)` is the plain return
  value of a tool without errors. `summarize_text` is a harness stand-in.
- `tools/requests`: `revealedKey` is `revealed_key`.
- `tools/publish`: TS `description: '...'` is a `///` doc comment (the
  MoonBit SDK's only way to set a method description); `check.sh` exempts
  doc comments from the colon rule. The comment above it is the TS comment.
  MoonBit's leaf method returns a `Result` (starting an invocation can fail
  synchronously), so the start is `.unwrap()`ped as in `tools/custom`. TS
  `if (result.status === 'rejected') throw result.reason` is `guard result
  is Ok(found) else { fail("CRM lookup failed") }`: `@tool.ToolError[E]` is
  not a MoonBit error type and cannot be raised. `const { name, plan } =
  result.value.structured` is `let { name, plan } = found.structured`.
- `tools/secrets`: TS's `config: { stripeKey: s.secret(z.string()) }` is a
  `#derive.config` struct with `stripe_key : @config.Secret[String]` plus the
  `Config[SupportConfig]` constructor parameter; the highlighted line is the
  field. The secret is forwarded through the shim above (one extra call
  compared with TS). The method returns the tool client's result with
  `.unwrap()` (as `tools/custom` does).
- `artifacts/overview`: TS `openai.responses.create(...).output_text` is
  `@llm.generate_text(model=@llm.openai("gpt-4.1"), prompt=…)` (as in
  `agents/stream`).
- `artifacts/routes`: TS `init` is the `async fn BoardAgent::new(team)`
  constructor (identity `{ team }` → the `team` path variable). The page is
  a top-level `let board_page : String` written with MoonBit `#|` raw
  string lines; the four lines of HTML/JS after `#|` are byte-identical to
  the TS template literal (MoonBit joins `#|` lines with `\n` and adds no
  trailing newline, as the template has none).
- `artifacts/routers`: `const bytes = (text) => new TextEncoder().encode(text)`
  is `fn bytes(s : String) -> Array[Byte] { @utf8.encode(s).to_array() }`
  (`HttpHeader.value` is `Array[Byte]`). TS
  `request.path.match(/^\/exports\/([\w-]+)\.csv$/)?.[1]` + `if (... || !id)`
  is one `guard` with the string pattern `request.path is [.. "/exports/",
  .. id, .. ".csv"]` and `is_word(id)` (harness helper for `[\w-]+`). The
  natural MoonBit regex form, `lexmatch request.path { re"^/exports/" +
  (re"[\w\-]+" as id) + re"\.csv$" => … }`, compiles with moonc but the
  Golem code generator cannot parse it (see the tools gap above). The id is
  a `StringView`, hence `id.to_owned()`. `AgentStream.from<Uint8Array>([])`
  is `@schema.AgentStream::produce(async fn(_) { })`: the SDK has no
  from-array constructor.
- `artifacts/login`: `s.unstructuredBinary({ mimeTypes: ['application/pdf'] })`
  is the phantom `struct Pdf {}` with its `AllowedMimeTypes` impl (3 lines);
  `return undefined` is `None`; `fs.readFileSync` is `@fs.read_bytes(...)`
  plus `Bytes::from_fixedarray`; `{ tag: 'inline', val, mimeType }` is
  `Some(Inline(bytes~, mime_type=Some("application/pdf")))`.
- `agents/stream`: the TS comment above `returns:` sits directly above the
  `) -> @schema.AgentStream[String]` line.
- Every `mkdirSync('/dir', { recursive: true })` is
  `root.create_directory_at("dir") |> ignore` (WASI has no recursive mkdir;
  "already exists" is ignored).

## Known limits

- Proven: every display file passes the static rules, the Golem code
  generator accepts its annotations, it builds to wasm with the in-tree SDK,
  and each module links into a valid component for the SDK's `agent-guest`
  world.
- Not proven at runtime: nothing was deployed or invoked (including the
  router's 404 path, forwarding the export stream as a response body, the
  page written by the `BoardAgent` constructor and the PDF response). The model calls
  raise by design and the bash stub aborts. Webhook suspension, durable
  sleep, scheduled/cancelled calls, reflection results, permission-card
  derivation/installation/revocation, secret reveal, file exposure, router
  handling, auth, durable streams, read-only caching, MCP description
  exposure and imported MCP tool calls were not observed on a Golem server.
- The MCP tool client is a hand-written stand-in (the generator needs
  `golem build` against a live MCP server); its method signature follows
  the generator source and its MoonBit compile test, but was not produced
  by the generator here.
- The research/slack stubs are generated by the in-component generator, not
  by the cross-component bridge generator; the method signatures match, the
  internal codecs differ.
- The Secrets snippet's tool call goes through the shim, so forwarding a
  config secret to a tool is not proven against the real SDK (it cannot be:
  see the SDK gap above).
- Compiler warnings (unused fields such as `ticket`/`project`, unused
  `self`, structs never constructed for tool structs, and warnings in
  generated code) are expected and do not fail the build.
