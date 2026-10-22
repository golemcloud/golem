# Rust home-page snippets: how they are compiled and checked

The Rust ports of the per-feature home-page snippets live in
`website/snippets/<chapter>/<feature>/rust/*.rs`, one file per feature
(21 in all, seven per chapter: every TypeScript feature; Rust has embedded
SQLite, so `agents/sqlite` is ported, and `tools/publish` is a code snippet,
so nothing in this harness reads YAML).

A line ending in ` //@` is highlighted on the page while its snippet shows;
the page strips the marker, and so does this harness before every rule and
compile step (it never counts toward the 68-character limit). Each Rust
file marks the line(s) corresponding to every marked TypeScript line.

| Chapter / feature | Display file | Agent / tool |
|---|---|---|
| agents/overview | `triage_agent.rs` | `TriageAgent` |
| agents/sqlite | `notes_agent.rs` | `NotesAgent` |
| agents/models | `draft_agent.rs` | `DraftAgent` |
| agents/team | `planner_agent.rs` | `PlannerAgent` |
| agents/waits | `checkout_agent.rs` | `CheckoutAgent` |
| agents/stream | `writer_agent.rs` | `WriterAgent` |
| agents/readonly | `stock_agent.rs` | `StockAgent` |
| tools/overview | `digest_agent.rs` | tool `summarize`, `DigestAgent` |
| tools/custom | `build_agent.rs` | tool `outline`, `BuildAgent` |
| tools/requests | `stripe_refund.rs` | `stripe_refund` helper |
| tools/permissions | `orders_access.rs` | `share_orders`, `revoke_orders` |
| tools/secrets | `support_agent.rs` | `SupportAgent` (config `stripe_key`), calls tool `refund` |
| tools/publish | `account_agent.rs` | `AccountAgent`, calls MCP-imported tool `crm_find_customer` |
| tools/middleware | `refund_policy.rs` | typed middleware `refund-policy` wrapping tools/secrets' `refund` |
| artifacts/overview | `analyst_agent.rs` | `AnalystAgent` (model tool `chart`) |
| artifacts/routes | `app_tool.rs` | tool `present`, `AppAgent` |
| artifacts/files | `chart_agent.rs` | `ChartAgent` |
| artifacts/login | `private_charts_agent.rs` | `PrivateChartsAgent` |
| artifacts/peruser | `dashboard_agent.rs` | `DashboardAgent` |
| artifacts/routers | `exports_router.rs` | raw HTTP router `ExportsRouter`, calls `AnalystAgent.export_csv` |
| artifacts/signin | `billing_agent.rs` | `BillingAgent` |

## Re-running

```shell
cd .amp/in/snippets/rust
./check.sh   # style + cargo check/build wasm32-wasip2 + validate + native registration test; ends ALL_OK
./neg.sh     # one planted Golem API misuse per snippet, each must fail; ends NEG_OK
```

Both print every command and its exit code. To compile by hand:
`cd check && WASI_SDK_PATH=$PWD/../.wasi-sdk cargo check --target wasm32-wasip2`.

## Prerequisites and versions

- macOS arm64 (darwin 25.6); rustup with Rust 1.95.0 stable and the
  `wasm32-wasip2` target (`rustup target add wasm32-wasip2`).
- Python 3 (3.14.5 used) for `style.py` and the `neg.sh` mutator.
- wasi-sdk 25.0 (wasi-libc 574b88da4815, LLVM 19), needed only to compile the
  bundled SQLite C source in `libsqlite3-sys` for `wasm32-wasip2`:

  ```shell
  curl -LO https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-25/wasi-sdk-25.0-arm64-macos.tar.gz
  tar xzf wasi-sdk-25.0-arm64-macos.tar.gz
  mv wasi-sdk-25.0-arm64-macos .amp/in/snippets/rust/.wasi-sdk
  ```

  (Use the `x86_64-linux`/`arm64-linux` archive elsewhere.) Here `.wasi-sdk`
  is a symlink to an earlier download, `../../hover-plan/calib/rust/.wasi-sdk`.
  `check.sh` and `neg.sh` export `WASI_SDK_PATH` themselves; without it cc-rs
  uses the host clang and `sqlite3.c` fails for `wasm32-wasip2`.
- Optional: `wasm-tools` 1.247.0 for component validation (skipped if absent).
- Network on the first build: crates.io plus two git dependencies (golemcloud
  forks of `rusqlite` and `wit-bindgen`). `check/Cargo.lock` pins everything.
- No SDK build step: the harness depends on the in-tree
  `sdks/rust/golem-rust` by path, which pulls the root-workspace
  `golem-rust-macro`, `golem-schema` and `golem-tool-metadata` by path. No
  root `Cargo.toml` is touched; `check/Cargo.toml` has an empty `[workspace]`.
  A cold build takes several minutes (the native test build adds a few more);
  warm re-checks take seconds.

## Harness layout

```
.amp/in/snippets/rust/
  check.sh            style.py, cargo check, cargo build, validate, native test; ALL_OK
  neg.sh              negative controls from neg-cases.tsv; NEG_OK
  neg-cases.tsv       path <TAB> reason <TAB> from <TAB> to [<TAB> from <TAB> to]
  style.py            display-file rules (see below)
  .wasi-sdk -> ../../hover-plan/calib/rust/.wasi-sdk
  check/
    Cargo.toml        standalone cdylib crate with its own [workspace]
    Cargo.lock
    build.rs          discovers display files, strips markers, generates $OUT_DIR/snippets.rs
    src/lib.rs        shared modules + include of snippets.rs + native test
    src/error.rs      AppError, the error type the display files propagate with `?`
    src/llm.rs        helpers around the real golem-ai-llm data model
    src/stubs.rs      stand-ins for generated bridge and tool clients
    src/prelude/<chapter>_<feature>.rs   one prelude per display file
```

### Dependencies (`check/Cargo.toml`)

- `golem-rust` (path `../../../../../sdks/rust/golem-rust`) with
  `export_golem_agentic`.
- `wit-bindgen` from `https://github.com/golemcloud/wit-bindgen`, branch
  `wit-bindgen-golem-1.6`, `=0.59.0`, features `async`, `async-spawn` (the
  fork the SDK uses; macro output needs it in the consumer crate).
- `golem-ai-llm`, `golem-ai-llm-openai`, `golem-ai-llm-anthropic` `=0.5.4`,
  all with `default-features = false`. Their default `golem` feature pulls
  `golem-rust =3.0.0-rc5` from crates.io with its own agent exports, which
  clash with the in-tree SDK. With it off the providers send through
  `golem-ai-http` (wasi:http 0.3) and take `OpenAiConfig::from_env()` /
  `AnthropicConfig::from_env()`.
- `rusqlite 0.38.0` (`bundled`, `wasm32-wasi-vfs`), with `rusqlite` and
  `libsqlite3-sys` patched to `https://github.com/golemcloud/rusqlite` rev
  `40024c3f7a2e1dfc2b36aac33e3d84ee04bfb700` (Golem fork with a WASI VFS).
- `regex 1` (1.13.1 locked; artifacts/routers' path pattern, as TS's
  regex literal; `AppError` converts from `regex::Error`).
- `serde`, `serde_json`, `serde_urlencoded 0.7.1`, `http =1.3.1` (the
  version golem-rust already uses; only for `http::Error`, the request
  builder's error, in `AppError`).

## How display files are spliced

`build.rs` runs on every `cargo check/build/test`:

1. Walks `website/snippets/*/*/rust/*` (sorted) and, for each file, requires
   `src/prelude/<chapter>_<feature>.rs` to exist.
2. Strips highlight markers from each display file: on every line a
   trailing run of spaces followed by `//@` or `#@` is removed, exactly what
   `sed -E 's,[ ]+(//|#)@$,,'` does, and the result is written to
   `$OUT_DIR/display/<chapter>_<feature>.rs` (same line numbers, so compiler
   errors point at the right line; their path is the stripped copy).
3. Writes `$OUT_DIR/snippets.rs` with one module per display file:

   ```rust
   #[allow(dead_code, unused_imports)]
   mod agents_overview {
       include!("<repo>/.amp/in/snippets/rust/check/src/prelude/agents_overview.rs");
       include!("<OUT_DIR>/display/agents_overview.rs");
   }
   ```

   Each snippet sits in its own module, so names cannot leak between
   snippets; `src/lib.rs` includes the file.
4. Emits `rerun-if-changed` for the snippets tree, every chapter, feature and
   `rust/` directory and every display file, so new files and edits are seen.
   It also prints `cargo:warning=snippet <rel> -> mod <module>` for each file
   found, as a visible inventory.
5. If `SNIPPET_NEG=<chapter>/<feature>/rust/<file>=<path>` is set (only by
   `neg.sh`), that one module uses `<path>` (stripped the same way) instead;
   the build script fails if the value matches no display file.

### One build

Earlier, agents/overview and tools/secrets both defined `SupportAgent`, which
needed two builds. agents/overview is now `TriageAgent`, so every snippet is in
one component and one build (the `secrets` feature is gone).

Snippets whose TypeScript shows imports (`models`, `sqlite`, `waits`,
`publish`) show the corresponding `use` lines; the preludes for them do not
import those names again. All other snippets omit `use` lines, as the
TypeScript does.

## What check.sh does

1. `python3 style.py` reads every display file, strips its markers (same
   regex), and fails on: a line
   over 68 characters, a tab, trailing whitespace, a block comment, a comment
   not on its own line (string and char literals are masked first, so
   `https://` is fine), a comment not ending with `:`, a file over 31 lines,
   a feature with a TypeScript original but no Rust file (or vice versa),
   a banned phrase ("deny by default", "stopped", "as if nothing
   happened", "variables persist", "storage") anywhere in the file,
   and comments whose wording or order differs from the TypeScript original
   (`agents/team` expects `from a TypeScript ResearchAgent` instead of `from a
   Rust ResearchAgent`, so the Rust call stays cross-language; the TypeScript
   original is stripped of markers too), and a file with no marker whose
   TypeScript original has some. It prints each file's line count, longest
   line, marker count and the TypeScript marker count. (A Rust file may have
   more markers than its original when one TypeScript line maps to several
   Rust lines; that correspondence is checked by eye.)
2. `cargo check --target wasm32-wasip2`, then `cargo build` (links the
   component), `test -s` on the `.wasm`, and
   `wasm-tools validate --features all` (component-model async streams need
   `all`), then
3. `cargo test --lib -- --nocapture` on the host. The test evaluates every
   registered agent, router and tool descriptor, which runs the SDK's
   registration-time HTTP validation (`validate_wire_agent_http`: mount
   variables vs constructor parameters, endpoint variables, principals,
   auth/CORS) that the wasm build only runs at component start. It asserts
   the 19 agent types (including the router `ExportsRouter`, registered
   with kind `HttpRouter`) and the `outline` and `present` (both
   `requires_filesystem`; `present`'s descriptor carries the slug regex
   `^[a-z0-9-]{1,40}$`), `refund` (version `1.0.0`) and `summarize` tools
   are registered; that the middleware `refund-policy` is registered,
   presents `refund` with a result carrying `customer_email`, and has no
   `caller` input (the principal is auto-injected); that `AccountAgent.describe` carries the description
   `Summarize a customer account by email` and `ChartAgent.chart` the
   description `Draw a sales chart for one quarter`; that `StockAgent.level` is read-only; and it decodes
   `SupportAgent`'s descriptor and asserts a config declaration with path
   `["stripe_key"]` and source `Secret` (the Rust macro declares the field
   name as written; the platform canonicalises secret paths to camelCase,
   `stripeKey`: `golem-common/src/base_model/agent_secret.rs`,
   `CanonicalAgentSecretPath`).
4. Prints `ALL_OK` only if every step exited 0.

## Errors: no `.unwrap()` in display files

No display file calls `.unwrap()`. Agent methods, tool methods and plain
functions that can fail return `Result` and propagate with `?`:

- `AppError` (`src/error.rs`, not displayed) is the shared error type:
  `#[derive(Debug, ToolError)] enum AppError { #[tool_error(kind =
  "runtime-error", exit_code = 1)] Failed(String) }`. The `ToolError`
  derive implements `IntoSchema`/`FromSchema` (and the direct wire traits),
  so it is valid both as an agent method error (`-> Result<Plan,
  AppError>`, encoded as a Golem result) and as a tool error (tools/custom's
  `outline`). It converts (`From`) from `String`/`&str`, `std::io::Error`,
  `rusqlite::Error`, golem-ai-llm `Error`, `serde_urlencoded::ser::Error`,
  `golem_rust::http::Error`, `http::Error`, `WebhookError`,
  `ConfigValueError`, `SecretAccessError`, `ToolReflectionError`,
  `GolemReflectError`, the agent `RpcError`, `PermissionError`,
  `LifespanError`, the stubs' `ClientError`, and any `ToolError<E>` (tool
  client failures, including the MCP-imported tool's `ToolError<E>` in
  tools/publish; `E` needs no `Debug`). The tools `summarize`, `outline`
  and (in the tools/secrets prelude) `refund` all use it as their error.
- Constructors must return `Self` (the macro only recognises `-> Self`;
  `golem-rust-macro/src/agentic/helpers.rs:18`), so they do no fallible
  work. `NotesAgent` opens SQLite lazily: a displayed `fn open() ->
  Result<Connection, AppError>` (create `/data`, open, create the table) is
  called by each method; the agent struct is a unit struct.
- `PrivateChartsAgent.open` returns `Option<Png>`: `None` is the 404 (TS
  `undefined`). Its only fallible step is the asker key; see
  "Deviations".
- The `chart` tool body in artifacts/overview is an async closure whose
  `?` propagates `AppError`; the loop helper turns an `Err` into a tool
  failure sent back to the model (as the AI SDK does with a throwing
  `execute`).
- `spawn_local` takes `Future<Output = ()>`, so a stream producer task
  (agents/stream) cannot use `?`, and a bare
  `AgentStream` has no terminal error payload; per the SDK streaming guide
  a producer panic fails the active invocation. There, a failed
  `write_one` (reader dropped) ends the task with `if ...is_err() { return;
  }` (the guide's pattern), and other failures use `.expect("...")`.
- The model helpers `text`/`json` return `Result<_, AppError>` (model
  errors, truncation, filters, schema mismatch) and are called with `?`.

## Preludes (what each snippet omits)

| Prelude | Supplies |
|---|---|
| every prelude | `use crate::error::AppError;` |
| `agents_overview` | `Schema`, `HashMap`; `classify` (TS `classify`, an HTTP classifier returning `Label`) and `apply_label` (TS `applyLabel`, an HTTP POST) |
| `agents_sqlite` | `agent_definition`/`agent_implementation`, `std::fs` |
| `agents_models` | `crate::llm` helpers, `Schema` |
| `agents_team` | generated-client stubs, `reflection`, `Plan`, `in_days` |
| `agents_waits` | imports; `PaymentEvent`; a real `MailerAgent` (so `MailerAgentClient` is the SDK-generated client); `in_days`; `create_payment_link`, `send_survey` (returning `Result<(), AppError>`) |
| `agents_stream` | `crate::llm` helpers, OpenAI provider, `AgentStream`, `spawn_local` |
| `agents_readonly` | `read_only` |
| `tools_overview` | `http`, `Request`, `type Url = String`, `type Urls = Vec<Url>` (TS `Urls`), `summarize_text` (TS `summarizeText`), and the unit structs `SummarizeImpl` and `DigestAgentImpl` (omitted for height, as in tools/custom) |
| `tools_custom` | bash client stub, `Headings = Vec<String>`, and the unit structs `OutlineImpl` and `BuildAgentImpl` (omitted from the display file to fit 31 lines) |
| `tools_requests` | `http`, `Request`, `AUTHORIZATION`, `CONTENT_TYPE` |
| `tools_permissions` | `AgentId`, `permissions::{self, Bounds, Holder, Lifespan, PatternGrant, PersistentCard}` |
| `tools_secrets` | `Config`, `Secret` (`golem_rust::agentic::Secret<T>`), `ConfigSchema`; the real `refund` tool (`#[tool_definition(version = "1.0.0")] trait Refund`, `api_key: GuestSecretHandle`, an auto-injected `caller: golem_rust::tool::Principal` that is not tool input and that `RefundClient` does not take, reveals the key and POSTs to Stripe with `expand[]=charge`), its `Receipt { id, status, customer_email }` result (pub fields; `customer_email` is the charge's billing email) and so the real macro-generated `RefundClient`, `RefundMiddleware` and `RefundUnderlying` |
| `tools_middleware` | `use crate::tools_secrets::{Receipt, RefundMiddleware, RefundUnderlying}` (the same tool, not a second `refund`); `GuestSecretHandle`, `tool::{Principal, ToolInvokeError}`, `PromiseId`, `create_promise`, `await_promise`, `tool_middleware`; the unit struct `RefundPolicy` with `new()` (height); `ask_finance` (TS `askFinance`: POSTs the promise id, charge and amount to finance), `approved` (TS `approved`: JSON `approved === true`), `check_rate_limit` (TS `checkRateLimit`, returns `ToolInvokeError::ConstraintViolation` over the limit), `audit` |
| `artifacts_peruser` | agent macros, `Uuid` (`golem_rust::Uuid`), `fs`; `dashboard_for` (TS `dashboardFor`) |
| `artifacts_routers` | `AgentStream`, `Config`, `Header`, `HttpRequest`, `HttpResponse`, `HttpRouter`, `http_router`, `agent_client`, `Regex`; the caller-owned client `#[agent_client(type_name = "AnalystAgent")] trait AnalystAgent { fn new(chat: String) -> Self; fn export_csv(&self, id: String) -> AgentStream<Vec<u8>>; }` (TS prelude `Analyst`; generates the real `AnalystAgentClient` without defining a second `AnalystAgent` in this component); `header` and `empty` (TS helpers); `failed` (an error becomes a 500 with an empty body) |
| `artifacts_signin` | `Principal`, agent macros; `Invoice`, `invoices_of`, `page_for` (TS helpers) |
| `tools_publish` | `agent_definition`/`agent_implementation`, `description`; `use crate::stubs::crm_find_customer_tool_guest_client;` so the displayed `use crm_find_customer_tool_guest_client::CrmFindCustomerClient;` resolves to the stand-in |
| `artifacts_overview` | `model`, `run_tool_loop` (`crate::llm`), golem-ai-llm `ToolDefinition`, `fs`; `Quarter` (TS `Quarter`: a newtype with `SCHEMA`, the JSON Schema with `pattern` `^Q[1-4]-\d{4}$`, and `ToolInput::parse`, which rejects anything else before the tool body runs, so only a valid quarter reaches a path); `chart_png` (TS `chartPng`); `page_for` (TS `pageFor`, same HTML) |
| `artifacts_routes` | `Schema`, agent and tool macros, `fs`; `Row` (TS `Row`), `type Rows = Vec<Row>` (TS `z.array(Row)`, so the impl line fits 68 columns), `query_sales` (TS `querySales`); the unit structs `PresentImpl` and `AppAgentImpl` (height) |
| `artifacts_files` | `UnstructuredBinary`, `AllowedMimeTypes`, `description`, `endpoint`; `SaleRow`, `query_sales`, `draw_chart` (TS helpers) |
| `artifacts_login` | `Principal`, `UnstructuredBinary`, `AllowedMimeTypes`, `endpoint`; `Chart { asked_by, png }` (TS `Chart`), `answer_with_chart` (TS `answerWithChart`), `user_key` (TS `userKey`: JSON `[issuer, sub]` of an OIDC principal, an error otherwise) |

## Invented libraries and stubs

**Invented libraries: none.** Every library is real and compiles for
`wasm32-wasip2`: in-tree `golem-rust`; `golem-ai-llm`,
`golem-ai-llm-openai` and `golem-ai-llm-anthropic` 0.5.4 (crates.io) for the
model calls (models, stream, artifacts/overview); the Golem `rusqlite` fork
for SQLite.

**Helpers (`src/llm.rs`, not a library)** over the real golem-ai-llm data
model. `run_tool_loop` (artifacts/overview; TS `generateText({ model,
prompt, tools: { chart }, stopWhen: stepCountIs(5) })`) is the labelled
model-loop helper. golem-ai-llm has no loop of its own (its `tools`
module "does not call an LLM, approve calls, or run a conversation loop",
`golem-ai-llm-0.5.4/src/tools.rs:1-7`, and is behind the `golem` feature,
`src/lib.rs:6`). The helper puts the displayed `ToolDefinition` in
`Config::tools`, calls `OpenAI::send`, parses each `Response::tool_calls`
entry with `ToolInput::parse`, awaits the displayed tool closure, sends
`Event::Response` and `Event::ToolResults` back, and returns the reply
text once no tool is called (at most five model steps). The other helpers
let the display code call the real `OpenAI::send` / `Anthropic::send`
without spelling out every `Config` field: `user(text)`, `assistant(text)`
(message `Event`s), `model(name)` (a plain `Config`), `json_model::<T>(name)`
(a `Config` whose `provider_options` carry a JSON Schema rendered from `T`'s
Golem schema, as OpenAI `text.format` and Anthropic `output_format`),
`text(result)` (the single text part; errors on truncation, filters) and
`json::<T>(result)` (decodes the reply against `T`'s Golem schema).

**Stubs (`src/stubs.rs`)** type-check callers and `unimplemented!()` at
runtime:

- `bash_tool_guest_client::BashClient` (`new()`, `run(script, cwd,
  timeout) -> Result<BashResult, ToolError<Infallible>>`): the client
  `golem build` generates for the built-in `bash@0.2.1` release.
- `research_agent_guest_client::{ResearchAgent, Facts}` and
  `slack_agent_guest_client::SlackAgent`: guest clients for agents in other
  components (`get(..) -> Result<Self, ClientError>`, awaited methods,
  `trigger_*`, `schedule_*`, `schedule_cancelable_*`), shaped after
  `cli/golem-cli/src/bridge_gen/rust/mod.rs` (guest mode). The comment calls
  ResearchAgent a TypeScript agent, so the call stays cross-language.
- `crm_find_customer_tool_guest_client::{CrmFindCustomerClient,
  CrmFindCustomerError, CrmFindCustomerResult, Structured, Content}`:
  **stand-in for the generated client** of the MCP-imported tool
  `crm_find_customer` (input `{ email: string }`, `additionalProperties:
  false`; output schema with string `name` and `plan`). The shape follows
  `golem-mcp-import/src/tool.rs` (one root command; input properties become
  options, so a required `email` is one `String` parameter and, with
  `additionalProperties: false`, there is no extras parameter; optional
  stdout; result record `{ structured, content }` whose `structured` has the
  output schema's type; one error case `mcp-tool-error` with a string
  payload), `cli/golem-cli/src/bridge_gen/rust/tool.rs` (`<Tool>Client::new()`;
  an `async fn <tool>(&self, ..)` that, because the command has stdout,
  returns `Result<golem_rust::agentic::ToolInvocation<R, E>,
  ToolError<E>>`; error enum `<Tool>Error`), the docs
  (`docs/src/content/v1.6/invoke/mcp.mdx`, "Call the imported tool", Rust
  tab: `Client::new().tool(..).await?` then `invocation.collect().await`)
  and the CLI test `cli/golem-cli/tests/app/mcp_import.rs`
  (`BearerFreshClient::new().bearer_fresh(..).await...collect().await.result...structured.generation`).
  `ToolInvocation`, `collect()` and `CollectedToolInvocation` are the real
  SDK types (`sdks/rust/golem-rust/src/agentic/tool_client.rs:901-964`), so
  only the client struct and the result/error types are stand-ins. The
  generated names of the anonymous result records are guesses and are not
  displayed (the snippet reads `result.structured` and its fields);
  `content` is reduced to a text case.

Generated clients for agents in the same crate (`MailerAgentClient`)
and for tools defined with `#[tool_definition]`
(`OutlineClient`, `RefundClient`, `SummarizeClient`) are the real
macro-generated ones.

**Shims: none.** Method descriptions (`#[description("...")]`, macro in
`golem-rust-macro/src/lib.rs:66`, read by `extract_description` in
`golem-rust-macro/src/agentic/agent_definition_impl.rs:807`) and the typed
`structured` field are both real; the registration test checks the
description in the descriptor.

## Deviations from the TypeScript, and why

- agents/overview: TS `Label` (`z.enum`) is a `#[derive(Schema)]` unit enum
  and the `z.record` result is `HashMap<String, Label>`; `classify` returns a
  typed `Label`, so there is no separate `Label.parse`. The constructor
  `fn new(inbox: String)` (TS `id`) is unmarked, as in TS.
- agents/team: the comment names a TypeScript ResearchAgent (not Rust). Each
  TS marked statement spans two Rust lines (`?` and `.await` chains,
  `collect::<Vec<_>>().join`), so Rust marks 9 lines for TS's 5. Tool types
  expose `lookup_name()` (there is no `name()`).
- agents/readonly: no HTTP; `#[read_only]` with the three TS comments above
  it. `z.int()` is `u64`.
- tools/overview: TS `summarize.client().summarize({ url })` is the
  macro-generated `SummarizeClient::new().summarize(url)`. TS `fetch(url)` is
  `golem_rust::http::send` with a `Request::get(url)` built on the line before
  the comment. The parameter type is `Url` (a prelude alias of `String`,
  beside TS's `Urls`) so both signatures fit 68 columns; the impl structs are
  in the prelude (height).
- tools/custom: `OutlineImpl`/`BuildAgentImpl` unit structs live in the
  prelude (height). The `requires_filesystem` comment sits above the
  `#[tool_definition(requires_filesystem = true)]` line. `outline` is a
  sync tool method returning `Result<Headings, AppError>` (TS returns
  `ok(...)`); `async` would push the signature past 68 columns. A non-zero
  bash exit returns `Err(run.stderr.into())` where TS throws.
- tools/requests: `revealedKey` is `revealed_key` (Rust naming); 2 marked
  lines (the request builder and `http::send`) for TS's one `fetch`.
- tools/publish: shows the `use` line for the generated crate, as the TS
  shows its `import` (the brief's no-imports rule conflicts with the final TS
  here; the TS wins, as for models, sqlite and waits). The rejected-result
  `throw` is the `?` on `.result`. TS destructures
  `const { name, plan } = ...structured`; Rust binds `customer =
  result.structured` (marked) and then the two fields, because the generated
  record's type name is not known offline.
- artifacts/overview: TS `tool({ description, inputSchema: Quarter,
  execute })` is a real golem-ai-llm `ToolDefinition { name, description,
  parameters_schema }` (marked on its first line) plus the tool body as an
  async closure `async |Quarter(q)| { ... }`, passed with the question and
  `model("gpt-4.1")` to the labelled prelude loop `run_tool_loop` (TS
  `generateText`/`stepCountIs(5)`); a full golem-ai-llm loop does not fit
  31 lines. The closure's `create_dir_all` and the two marked writes are
  visible. `exposeFiles: [{ route: '/*', path: '/out/$1' }]` is
  `filesystem_bindings = [("/*", "/out/$1")]`, with the comment between
  `mount` and the bindings. The tool returns the page path as a string;
  the loop JSON-encodes it as the tool result.
- artifacts/routes: TS `toolDefinition('present', { requiresFilesystem })
  .body(b => b.positional('name', Name).positional('html', ...))` is
  `#[tool_definition(requires_filesystem = true)] trait Present` with
  `fn present(&self, name: String, html: String)`. Scalar parameters are
  positional by default (`golem-rust-macro/src/tool/descriptor.rs:1206-1219`),
  and TS's `Name` regex is the real `#[arg(name, regex = "...")]` text
  refinement (`golem-rust-macro/src/tool/arg.rs:83`), which the executor
  enforces on tool input (`golem-worker-executor/src/durable_host/tool/command.rs:236`,
  `validate_value`; regex check in `golem-schema/src/schema/validation/value.rs:552`).
  So the regex is on screen, where TS keeps `Name` in its prelude. TS
  `ok(...)` is `Ok(...)`; `z.array(Row)` is the prelude alias `Rows`; the
  impl unit structs are in the prelude (height).
- artifacts/files: TS `s.unstructuredBinary({ mimeTypes: ['image/png'] })`
  is `type Png = UnstructuredBinary<PngType>` with a
  `#[derive(AllowedMimeTypes)]` enum. The "reply is the PNG" comment sits
  between the `fn chart(...)` line and its marked `-> Result<Png,
  AppError>` line. TS's one-line `drawChart(await querySales(...))` is two
  lines (query, then the marked draw). `{ tag: 'inline', val, mimeType }`
  is `Png::from_inline(data, PngType::Png)` (marked).
- artifacts/login: the same `Png` type, with no blank line after it
  (height). The mount is one unmarked line
  `#[agent_definition(mount = "/private/{team}", auth = true)]`, as TS. `z.number()` is `u32`. TS `this.charts[n - 1]` is
  `self.charts.get(n.checked_sub(1)? as usize)?`, so `n = 0` is a 404. TS
  `userKey(caller)` throws for a non-OIDC principal in both methods; in
  Rust `ask` propagates that error with `?`, and `open` returns `None`
  (404) through `.ok()?`, because `open` returns `Option<Png>` to fit 31
  lines. With `auth = true` every caller is an OIDC user, so the two only
  differ for a principal that cannot reach this mount.
- agents/stream: the "clients resume this Durable Stream" comment sits
  between `#[endpoint(post = "/draft")]` and the `fn` whose
  `AgentStream<String>` return type it describes. Streams are produced with
  `AgentStream::new()` and `spawn_local`, the SDK's documented producer
  pattern.
- agents/sqlite: SQLite is opened lazily by a displayed `fn open()` (see
  "Errors"); the comment sits above `Connection::open`, after
  `create_dir_all`, as in TS.
- tools/secrets: TS's inline `config: { stripeKey: s.secret(...) }` is a
  `SupportConfig` struct; the "opaque handle" comment sits above the
  secret field. The result type is `Receipt` (the `refund` tool's trait is
  already named `Refund`).
- tools/middleware: TS `refund.middleware({ name, implementation })` is
  `#[tool_middleware(name = "refund-policy", constructor =
  RefundPolicy::new)] impl RefundMiddleware for RefundPolicy` (both the
  attribute's first line and the `impl` line marked for TS's one line);
  the unit struct is in the prelude (height). TS gets `principal` from the
  middleware context; the Rust typed middleware receives only the tool's
  own parameters, so the `refund` tool declares an auto-injected
  `caller: Principal` parameter (filled by Golem, not part of the tool
  input, not taken by `RefundClient` or `RefundUnderlying::refund`) and the
  middleware reads it. TS `refuse` builds `new ToolInvokeError({ tag:
  'constraint-violation' })`; Rust `refuse` returns
  `ToolInvokeError::ConstraintViolation`. TS `throw` is `return Err(..)`.
  TS `createPromise()`/`awaitPromise(p)` are `create_promise()` and
  `await_promise(&p).await` (bytes). TS `underlying.refund(args)` is
  `underlying.refund(charge.clone(), amount, api_key).await?` (two lines;
  `charge` is reused by `audit`). TS `{ ...receipt, customerEmail }` is
  struct update syntax. `ask_finance`/`audit` errors (`AppError`) become
  `ToolInvokeError::Tool` through `?`.
- artifacts/peruser: TS `crypto.randomUUID()` is `Uuid::new_v4()`
  (`golem_rust::Uuid`, the `uuid` crate re-exported at
  `sdks/rust/golem-rust/src/lib.rs:18`, `v4` feature enabled in
  `sdks/rust/golem-rust/Cargo.toml:90`; the SDK's own `new_phantom` uses
  it, `docs/src/content/v1.6/how-to-guides/rust/golem-multi-instance-agent-rust.mdx:96`).
  TS `auth: true` is the unmarked `auth = true,` line of the mount, as in
  artifacts/login and artifacts/signin. TS `init` is `fn new(user)`
  (marked). `ask` keeps `&mut self` though it no longer mutates state.
- artifacts/routers: TS `defineHttpRouter('ExportsRouter').mount('/exports')
  .implementRaw(...)` is `#[http_router(name = "ExportsRouter", mount =
  "/exports")] impl HttpRouter for ExportsRouter` with `type Config = ()`
  and `fn new` (required trait items) and a displayed `struct
  ExportsRouter;`. `HttpRouter::handle` returns `HttpResponse`, not a
  `Result`, so it calls a displayed `async fn export(request) ->
  Result<HttpResponse, AppError>` and maps an error to the prelude's
  `failed` (500, empty body) instead of `.unwrap()`; in TS the exception
  escapes the router. The regex is the `regex` crate (compiled per
  request; `Regex::new(..)?`). TS's `request.method !== 'GET' || !m` is a
  `let Some(m) = m.filter(|_| request.method == "GET") else { 404 }`.
  TS `Analyst.client.get({ chat }).exportCsv({ id })` is
  `AnalystAgentClient::get(chat)?.export_csv(id).await?` (the generated
  client from the prelude's `#[agent_client]` trait). The 200 response
  puts `status` and `headers: vec![` on one line and closes with `], body:
  csv })` (height); the `body` marker is on that closing line.
- artifacts/signin: TS `if (caller.tag !== 'oidc') throw` is `let
  Principal::Oidc(user) = caller else { return Err(..) };`; TS `const {
  issuer, sub } = caller` is `let (issuer, sub) = (user.issuer, user.sub)`.
  The "passes in the user" comment sits between `#[endpoint(post =
  "/ask")]` and the marked `fn ask(.., caller: Principal)` line.
- agents/waits: TS `setTimeout as sleep` maps to
  `golem_rust::wasip3::clocks::monotonic_clock::wait_for as sleep` (ns).

## Negative controls (neg.sh)

For each line of `neg-cases.tsv`, `neg.sh` writes a mutated copy of one
display file to `neg/` (each from/to pair must occur exactly once; the copy
keeps its markers and build.rs strips them), runs `cargo check` with
`SNIPPET_NEG=<rel>=<copy>`, and requires a non-zero exit; it fails if any
display file has no case, then restores the positive build and deletes
`neg/`.

| Snippet | Planted misuse | Error |
|---|---|---|
| agents/models | drop `#[derive(Schema)]` from `Review` | E0277 `Review: IntoWire` |
| agents/overview | drop `Schema` from `Label`'s derive | E0277 `Label: FromWire`/`WireSchema` |
| agents/readonly | `#[read_only(cache = true)]` | macro: cache must be a string mode |
| agents/sqlite | add a `std::time::Instant` method parameter (trait and impl) | E0599 `AgentArgument<Instant>` bounds |
| agents/stream | `AgentStream::channel()` | E0599 |
| agents/team | `.gather()?` without `.await` | E0277 `?` on a future |
| agents/waits | `schedule_remind_cancelable` | E0599 on `MailerAgentClient` |
| artifacts/files | `Png::from_inline(data, "image/png")` | E0308 expected `PngType`, found `&str` |
| artifacts/login | owner check `made.asked_by != caller` (the raw `Principal`) | E0277 can't compare `String` with `Principal` |
| artifacts/overview | mount `/chats/{name}` | macro: filesystem mount capture `{name}` must bind one constructor parameter |
| artifacts/routes | `regex = 40` | macro: regex must be a string literal |
| tools/custom | `requires_filesystem = "yes"` | macro: must be a bool literal |
| tools/overview | `SummarizeClient::new().summarize(url, 280)` | E0061 generated tool client takes 1 argument |
| tools/permissions | `Holder::Worker` | E0599 no variant |
| tools/publish | `call.collect().result?` (no `.await`) | E0609 no field `result` on the real SDK's `collect()` future |
| tools/requests | body without `.into_bytes()` | E0308 `Request<Vec<u8>>` |
| tools/middleware | pass the auto-injected `caller` to `underlying.refund(..)` | E0061 the underlying handle takes 3 arguments, not 4 |
| artifacts/peruser | mount `/dashboards/{name}` | macro: filesystem mount capture `{name}` must bind one constructor parameter |
| artifacts/routers | drop `type Config = ();` | E0046 missing `Config` in `HttpRouter` impl |
| artifacts/signin | `user.subject` | E0609 no field `subject` on `OidcPrincipal` (it is `sub`) |
| tools/secrets | pass `config.stripe_key` (the `Secret<String>` wrapper) instead of `.handle()?` | E0308 expected `GuestSecretHandle`, found `Secret<String>` |

## Known limits (compiles; nothing is deployed or run)

- The component is linked and validated but never deployed or invoked.
  Registration-time validation runs natively; nothing else executes.
- `golem-ai-llm*` with default features off skip the crates' own durability
  wrapper; the claim "completed model calls are not repeated on recovery"
  then rests on Golem recording the underlying wasi:http 0.3 calls, which is
  untested here. The in-tree skill `golem-add-llm-rust` also says no golem-ai
  release is verified against this SDK with its Golem integration enabled.
  Whether OpenAI/Anthropic accept the `provider_options` JSON-schema keys is
  untested.
- The middleware's ordering, unbypassability and promise suspension, the
  router's 404/streamed CSV and `AnalystAgent.export_csv` (no AnalystAgent
  in this component has that method; the `#[agent_client]` client is
  checked against the trait only), and the OIDC-only branch of
  `BillingAgent.ask` are type-checked and registration-checked only.
- Generated cross-component clients (bash, `ResearchAgent`, `SlackAgent`,
  the MCP-imported `crm_find_customer`) are stubs, so their calls are checked
  only against the stub shapes (the MCP client's `ToolInvocation`/`collect`
  are real SDK types).
- SQLite file persistence under `/data`, durable `wait_for`,
  scheduled/cancelled reminders, secret reveal, permission-card
  derivation/installation/revocation, live-file exposure (`/*` →
  `/out/$1`), the model tool loop (whether OpenAI calls `chart` and
  accepts the `pattern` schema), the `present` tool writing into the
  calling agent's files, the regex refinement rejecting a bad name,
  read-only caching, and the PNG replies and private-chart 404 are
  type-checked (and, for HTTP shapes, registration-validated) only.
- The secret-to-tool path (config handle forwarded to `refund`) is
  type-checked and registration-checked only; no host checks that a
  binding lets `refund` reveal it.

## SDK notes for tools/secrets (read from source, compiled)

The TypeScript SDK bug (tool client types `s.secret` as the raw WIT
`Secret`, and the encoder adopts the config wrapper as a raw handle) does
not exist in Rust:

- `Secret<T>::handle()` (`sdks/rust/golem-rust/src/agentic/agent_config.rs:151-166`)
  calls `get_config_value` with a `secret<T>` schema and decodes a genuine
  owned handle into `GuestSecretHandle`; it never reveals.
- The tool's `api_key: GuestSecretHandle` parameter is what the
  generated `RefundClient::refund` takes; the `Secret<String>` config
  wrapper itself does not type-check there (the neg case proves it), and
  the encoder moves the handle out of its take-once cell
  (`golem-schema/src/schema/wit/mod.rs:536-586`).

One weaker spot, not a runtime bug for this snippet: `GuestSecretHandle` is
untyped and always declares `secret<string>`
(`golem-schema/src/schema/wit/mod.rs:621-633`, `SecretSpec::default()`,
and `golem-schema/src/schema/wit/direct.rs:1637-1645`), and `handle()`
erases `T`, so a `Secret<u64>` config handle would also type-check as a
string-secret tool argument. A fix would be a typed `SecretHandle<T>`
returned by `Secret<T>::handle()` and accepted by tool parameters, with the
schema taken from `T`.

## SDK APIs behind the new snippets (read from source, compiled)

- Typed middleware: `#[tool_middleware(name, constructor)]`
  (`golem-rust-macro/src/lib.rs:158`; keys and the required zero-argument
  constructor in `golem-rust-macro/src/tool/middleware_authoring.rs:97-153`).
  `#[tool_definition]` generates `<Tool>Middleware` and `<Tool>Underlying`
  (`golem-rust-macro/src/tool/middleware_surface.rs:31-32`); each middleware
  method is `async fn <cmd>(&self, underlying: &U, <tool params>) ->
  Result<T, ToolInvokeError<E>>` (`middleware_surface.rs:778-783`). A tool
  parameter of type `golem_rust::tool::Principal` is auto-injected
  (`golem-rust-macro/src/tool/definition.rs:864-887`), filled from the
  invocation principal for the middleware (`middleware_surface.rs:964-981`)
  and dropped from the underlying handle's arguments
  (`middleware_surface.rs:815-830`) and the tool client
  (`golem-rust-macro/src/tool/client.rs:1394-1417`). Rejections use
  `ToolInvokeError::ConstraintViolation`
  (`sdks/rust/golem-rust/src/tool/tool_middleware.rs:130-143`). Usage
  matches `docs/src/content/v1.6/how-to-guides/rust/golem-tools-middleware-rust.mdx`.
- Promises: `golem_rust::create_promise() -> PromiseId`
  (`sdks/rust/golem-rust/src/lib.rs:993`) and `async fn
  await_promise(&PromiseId) -> Vec<u8>` (`lib.rs:1066`), as in
  `docs/src/content/v1.6/develop/promises.mdx` (Rust tab).
- Raw router: `#[http_router(name, mount)]`
  (`golem-rust-macro/src/lib.rs:110`) on `impl HttpRouter`
  (`sdks/rust/golem-rust/src/agentic/http/router.rs:118-125`: `type
  Config`, `fn new(Config<..>) -> Self`, `async fn handle(&self,
  HttpRequest) -> HttpResponse`); `Header { name, value: Vec<u8> }`,
  `HttpRequest { method, path, .. }`, `HttpResponse { status, headers,
  body: AgentStream<Vec<u8>> }` (`router.rs:9-35`), as in
  `docs/src/content/v1.6/invoke/making-custom-apis/http-routers.mdx`.
  Router-to-agent client: `#[agent_client(type_name = "...")]`
  (`golem-rust-macro/src/lib.rs:118`; a `new` constructor requires
  `type_name`, `golem-rust-macro/src/agentic/agent_client_impl.rs:164`),
  documented in `docs/src/content/v1.6/develop/reflection.mdx`
  (`XClient::get(..)?` then `.method(..).await?`).
- OIDC principal: `Principal::Oidc(OidcPrincipal { issuer, sub, .. })`
  (the WIT `golem:agent/common` principal re-exported as
  `golem_rust::agentic::Principal`, `sdks/rust/golem-rust/src/agentic/mod.rs:15`).

No SDK gap was found for these four features: every call compiles against
the in-tree SDK without a shim.

## artifacts/overview: UUID file names (Oct 22 2026)

The chart tool names the PNG and the HTML page with one fresh UUID, as artifacts/peruser does, and `pageFor(q, name)` points the page's `<img>` at `<name>.png`. `let name = Uuid::new_v4();` (`golem_rust::Uuid`, imported in `prelude/artifacts_overview.rs`; `page_for` takes the `Uuid`). The PNG write is wrapped onto two highlighted lines to fit 68 columns. No comment explains the UUID (owner ruling). Re-verified: `check.sh` ALL_OK, `neg.sh` passing.
