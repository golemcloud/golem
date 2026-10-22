# Rust home-page snippets: how they are compiled and checked

The Rust ports of the per-feature home-page snippets live in
`website/snippets/<chapter>/<feature>/rust/*.rs`, one file per feature
(17 in all: every TypeScript feature; Rust has embedded SQLite, so
`agents/sqlite` is ported). The shared Tools YAML
(`website/snippets/tools/publish/golem.yaml`, and the superseded
`tools/secrets/golem.yaml`) is not Rust and is not checked here.

A line ending in ` //@` is highlighted on the page while its snippet shows;
the page strips the marker, and so does this harness before every rule and
compile step (it never counts toward the 68-character limit). Each Rust
file marks the line(s) corresponding to every marked TypeScript line.

| Chapter / feature | Display file | Agent / tool |
|---|---|---|
| agents/overview | `support_agent.rs` | `SupportAgent` |
| agents/language | `cart_agent.rs` | `CartAgent` |
| agents/sqlite | `notes_agent.rs` | `NotesAgent` |
| agents/models | `draft_agent.rs` | `DraftAgent` |
| agents/team | `planner_agent.rs` | `PlannerAgent` |
| agents/waits | `checkout_agent.rs` | `CheckoutAgent` |
| tools/overview | `refund_tool.rs` | tool `refund` |
| tools/custom | `build_agent.rs` | tool `outline`, `BuildAgent` |
| tools/requests | `stripe_refund.rs` | `stripe_refund` helper |
| tools/permissions | `orders_access.rs` | `share_orders`, `revoke_orders` |
| tools/secrets | `support_agent.rs` | `SupportAgent` (config `stripe_key`), calls tool `refund` |
| artifacts/overview | `site_agent.rs` | `SiteAgent` |
| artifacts/routes | `report_agent.rs` | `ReportAgent` |
| artifacts/routers | `exports_router.rs` | router `ExportsRouter` (calls `ExportAgent`) |
| artifacts/stream | `writer_agent.rs` | `WriterAgent` |
| artifacts/login | `notebook_agent.rs` | `NotebookAgent` |
| artifacts/readonly | `page_agent.rs` | `PageAgent` |

## Re-running

```shell
cd .amp/in/snippets/rust
./check.sh   # style + 2x (cargo check/build wasm32-wasip2 + validate + native registration test); ends ALL_OK
./neg.sh     # one planted Golem API misuse per snippet, each must fail; ends NEG_OK
```

Both print every command and its exit code. To compile by hand:
`cd check && WASI_SDK_PATH=$PWD/../.wasi-sdk cargo check --target wasm32-wasip2`
(add `--features secrets` for the build that contains tools/secrets).

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
    src/stubs.rs      stand-ins for generated bridge clients
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
- `serde`, `serde_json`, `serde_urlencoded 0.7.1`, `http =1.3.1` (the
  version golem-rust already uses; only for `http::Error`, the request
  builder's error, in `AppError`).
- Feature `secrets` (no dependencies): see "Two builds" below.

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
   snippets (a prelude may still name a sibling module explicitly:
   `tools_secrets` uses `crate::tools_overview::{RefundClient, Receipt}`);
   `src/lib.rs` includes the file.
4. Emits `rerun-if-changed` for the snippets tree, every chapter, feature and
   `rust/` directory and every display file, so new files and edits are seen.
   It also prints `cargo:warning=snippet <rel> -> mod <module>` for each file
   found, as a visible inventory.
5. If `SNIPPET_NEG=<chapter>/<feature>/rust/<file>=<path>` is set (only by
   `neg.sh`), that one module uses `<path>` (stripped the same way) instead;
   the build script fails if the value matches no display file or targets
   the file this build leaves out.

### Two builds

`agents/overview` and `tools/secrets` both define an agent type named
`SupportAgent`, as in TypeScript. One component cannot hold both: the SDK
registry (`sdks/rust/golem-rust/src/agentic/agent_registry.rs:169-184`)
silently keeps whichever registers last, so one descriptor would escape the
registration test. The default build therefore leaves out `tools/secrets`
and `--features secrets` leaves out `agents/overview` (build.rs reads
`CARGO_FEATURE_SECRETS`); every other snippet is in both. `check.sh` runs
check, build, validate and the native test for both; `neg.sh` adds
`--features secrets` for the tools/secrets case.

Snippets whose TypeScript shows imports (`models`, `sqlite`, `waits`) show
the corresponding `use` lines; the preludes for them do not import those
names again. All other snippets omit `use` lines, as the TypeScript does.

## What check.sh does

1. `python3 style.py` reads every display file, strips its markers (same
   regex), and fails on: a line
   over 68 characters, a tab, trailing whitespace, a block comment, a comment
   not on its own line (string and char literals are masked first, so
   `https://` is fine), a comment not ending with `:`, a file over 31 lines,
   a feature with a TypeScript original but no Rust file (or vice versa),
   and comments whose wording or order differs from the TypeScript original
   (`agents/team` expects `Go ResearchAgent` instead of `Rust ResearchAgent`;
   the TypeScript original is stripped of markers too), and a file with no
   marker whose TypeScript original has some. It prints each file's line
   count, longest line, marker count and the TypeScript marker count. (A
   Rust file may have more markers than its original when one TypeScript
   line maps to several Rust lines; that correspondence is checked by eye.)
2. For the default build and then `--features secrets`:
   `cargo check --target wasm32-wasip2`, then `cargo build` (links the
   component), `test -s` on the `.wasm`, and
   `wasm-tools validate --features all` (component-model async streams need
   `all`), then
3. `cargo test --lib -- --nocapture` on the host. The test evaluates every
   registered agent, router and tool descriptor, which runs the SDK's
   registration-time HTTP validation (`validate_wire_agent_http`: mount
   variables vs constructor parameters, endpoint variables, principals,
   auth/CORS) that the wasm build only runs at component start; it asserts
   the 15 agent/router types and the `refund` (version `1.0.0`) and `outline`
   (`requires_filesystem`) tools are registered. With `secrets` it also
   decodes `SupportAgent`'s descriptor and asserts a config declaration with
   path `["stripe_key"]` and source `Secret` (the Rust macro declares the
   field name as written; the platform canonicalises secret paths to
   camelCase, `stripeKey`: `golem-common/src/base_model/agent_secret.rs`,
   `CanonicalAgentSecretPath`). (Checked once by hand:
   changing `PageAgent`'s mount to `/pages/{pg}` compiles but makes this test
   panic in registration; the same change on a mount with
   `filesystem_bindings` is already a compile error.)
4. Prints `ALL_OK` only if every step of both builds exited 0.

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
  client failures; `E` needs no `Debug`, since tool error enums such as
  `Failure` have none).
- tools/overview keeps its own `Failure { Http }` tool error; the prelude
  maps `AppError` and `http::Error` into `Failure::Http`, so
  `stripe_refund(..).await?` and `response_json(res).await?` work.
- Constructors must return `Self` (the macro only recognises `-> Self`;
  `golem-rust-macro/src/agentic/helpers.rs:18`), so they do no fallible
  work. `NotesAgent` opens SQLite lazily: a displayed `fn open() ->
  Result<Connection, AppError>` (create `/data`, open, create the table) is
  called by each method; the agent struct is a unit struct. `ReportAgent`
  (artifacts/routes) must write its front-end when it is created, as the
  TypeScript `init` does, so its constructor uses `.expect("...")` on the
  two file-system calls (a panic fails agent creation, as a throw in TS
  `init` does); `build` returns `Result` and uses `?`.
- `HttpRouter::handle` returns a plain `HttpResponse`
  (`sdks/rust/golem-rust/src/agentic/http/router.rs`), so the router
  (artifacts/routers) has no `?`: an unmatched path returns 404, and the
  generated non-fallible `ExportAgentClient::csv()` panics on an RPC
  failure (the invocation fails, as an uncaught throw does in TS).
- `spawn_local` takes `Future<Output = ()>`, so a stream producer task
  (artifacts/overview, artifacts/stream) cannot use `?`, and a bare
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
| `agents_language` | imports; `Order`; `place_order` (POST to an orders API) |
| `agents_overview` | imports; `Reply` (`IntoSchema`/`FromSchema`); `Approval`; `notify_reviewer`, `refund_order` (HTTP POSTs returning `Result<(), AppError>`) |
| `agents_sqlite` | `agent_definition`/`agent_implementation`, `std::fs` |
| `agents_models` | `crate::llm` helpers, `Schema` |
| `agents_team` | generated-client stubs, `reflection`, `Plan`, `in_days` |
| `agents_waits` | imports; `PaymentEvent`; a real `MailerAgent` (so `MailerAgentClient` is the SDK-generated client); `in_days`; `create_payment_link`, `send_survey` (returning `Result<(), AppError>`) |
| `tools_overview` | secret `reveal` binding, `GuestSecretHandle as Secret`, `Receipt`, `string_type`/`string_value`, `stripe_refund` (returns `Result<Response, AppError>`), `From<AppError>`/`From<http::Error>` for `Failure` |
| `tools_secrets` | `Config`, `Secret` (`golem_rust::agentic::Secret<T>`), `ConfigSchema`; `Receipt` and the real macro-generated `RefundClient` from the `tools_overview` module |
| `tools_custom` | bash client stub, `Headings = Vec<String>`, and the unit structs `OutlineImpl` and `BuildAgentImpl` (omitted from the display file to fit 31 lines) |
| `tools_requests` | `http`, `Request`, `AUTHORIZATION`, `CONTENT_TYPE` |
| `tools_permissions` | `AgentId`, `permissions::{self, Bounds, Holder, Lifespan, PatternGrant, PersistentCard}` |
| `artifacts_overview` | `AgentStream`, `spawn_local`, `endpoint`, `fs`; `stream_page` (a small async part source standing for TS `streamPage`) |
| `artifacts_routes` | `agent_definition`/`agent_implementation`, `endpoint`, `fs`; `front_end(team)` (TS `frontEnd`), `render` |
| `artifacts_routers` | `Config`, `Header`, `HttpRequest`, `HttpResponse`, `HttpRouter`, `http_router`, `AgentStream`; a real `ExportAgent` (`new(id: String)`, `csv(&self) -> AgentStream<Vec<u8>>`), so `ExportAgentClient` is the SDK-generated client; `is_word` (the regex's `[\w-]`), `header(name, value)` and `empty()` (TS `header`/`empty`) |
| `artifacts_stream` | `crate::llm` helpers, OpenAI provider, `AgentStream`, `spawn_local` |
| `artifacts_login` | `Principal`, `endpoint` |
| `artifacts_readonly` | `endpoint`, `read_only` |

## Invented libraries and stubs

**Invented libraries: none.** Every library is real and compiles for
`wasm32-wasip2`: in-tree `golem-rust`; `golem-ai-llm`,
`golem-ai-llm-openai` and `golem-ai-llm-anthropic` 0.5.4 (crates.io) for the
model calls, including the two-provider call in `models`; the Golem
`rusqlite` fork for SQLite.

**Helpers (`src/llm.rs`, not a library)** over the real golem-ai-llm data
model, so the display code calls the real `OpenAI::send` / `Anthropic::send`
without spelling out every `Config` field: `user(text)`, `assistant(text)`
(message `Event`s), `model(name)` (a plain `Config`), `json_model::<T>(name)`
(a `Config` whose `provider_options` carry a JSON Schema rendered from `T`'s
Golem schema, as OpenAI `text.format` and Anthropic `output_format`),
`text(result)` (the single text part; panics on errors, truncation, filters)
and `json::<T>(result)` (decodes the reply against `T`'s Golem schema).

**Stubs (`src/stubs.rs`)**, shaped after the Rust bridge generator
(`cli/golem-cli/src/bridge_gen/rust/tool.rs` and `mod.rs`, guest mode); they
type-check callers and `unimplemented!()` at runtime:

- `bash_tool_guest_client::BashClient` (`new()`, `run(script, cwd,
  timeout) -> Result<BashResult, ToolError<Infallible>>`): the client
  `golem build` generates for the built-in `bash@0.2.1` release.
- `research_agent_guest_client::{ResearchAgent, Facts}` and
  `slack_agent_guest_client::SlackAgent`: guest clients for agents in other
  components (`get(..) -> Result<Self, ClientError>`, awaited methods,
  `trigger_*`, `schedule_*`, `schedule_cancelable_*`).

Generated clients for agents in the same crate (`MailerAgentClient`) and for
tools defined with `#[tool_definition]` (`OutlineClient`, `RefundClient`)
are the real macro-generated ones.

**Shims: none for tools/secrets.** The Secrets snippet uses only real SDK
API: `#[derive(ConfigSchema)]` with `#[config_schema(secret)] stripe_key:
Secret<String>`, a `#[agent_config] config: Config<SupportConfig>`
constructor parameter, `self.config.get()?.stripe_key.handle()?` (fetches
the opaque `GuestSecretHandle` from agent config without revealing it), and
`RefundClient::new().refund(charge, amount, api_key).await?`, the client
`#[tool_definition]` generates for the `refund` tool in tools/overview,
whose `api_key` parameter is a `GuestSecretHandle`.

## Deviations from the TypeScript, and why

- tools/overview: the result schema TS calls `Refund` is `Receipt`, because a
  Rust tool's name comes from its trait (`trait Refund` → tool `refund`). The
  `http` error is `enum Failure { Http }` with `#[tool_error(kind =
  "runtime-error", exit_code = 1)]`.
- tools/custom: `OutlineImpl`/`BuildAgentImpl` unit structs live in the
  prelude (height). The `requires_filesystem` comment sits above the
  `#[tool_definition(requires_filesystem = true)]` line. `outline` is a
  sync tool method returning `Result<Headings, AppError>` (TS returns
  `ok(...)`); `async` would push the signature past 68 columns. A non-zero
  bash exit returns `Err(run.stderr.into())` where TS throws.
- artifacts/login: TS throws; Rust returns `Err("not your notes")` from
  `-> Result<(), String>` (unchanged; it has no `.unwrap()`). The principal is a `Principal` parameter (host-
  injected; `Principal::Oidc(user)` with `user.sub`).
- artifacts/stream: the "clients resume this Durable Stream" comment sits
  between `#[endpoint(post = "/draft")]` and the `fn` whose
  `AgentStream<String>` return type it describes. Streams are produced with `AgentStream::new()` and
  `spawn_local`, the SDK's documented producer pattern.
- artifacts/routes: TS `exposeFiles` is `filesystem_bindings` on
  `#[agent_definition]`, with the comment between `mount` and the bindings;
  TS `init` is the constructor (see "Errors" for its `.expect`). The
  `/site` directory is created in the constructor, so `build` writes
  without creating it again, as in TS.
- artifacts/routers: TS `defineHttpRouter(..).mount(..).implementRaw(..)`
  is `#[http_router(name, mount)]` on `impl HttpRouter` with `handle`, the
  SDK's raw (canonical-envelope) handler; there is no separate Web-style
  handler in Rust. The regex `^/exports/([\w-]+)\.csv$` is
  `strip_prefix`/`strip_suffix` plus a non-empty `[\w-]` filter (three
  marked lines for TS's one); the method check and the 404 sit in a
  `let ... else` (the empty body is bound on its own line for width).
  TS `Export.client.get({ id }).csv()` is the macro-generated
  `ExportAgentClient::get(id.to_string()).csv().await`, which returns the
  remote `AgentStream<Vec<u8>>`; it is forwarded unread as the response
  body (`AgentStream` docs: "Forward an unread stream directly"). No shim.
  Rust marks 8 lines for TS's 6.
- artifacts/readonly: `#[endpoint(get = "/")]`, then the three TS comments,
  then `#[read_only]` (they sit above `readOnly: true` in TS).
- agents/sqlite: SQLite is opened lazily by a displayed `fn open()` (see
  "Errors"); the comment sits above `Connection::open`, after
  `create_dir_all`, as in TS.
- tools/secrets: TS's inline `config: { stripeKey: s.secret(...) }` is a
  `SupportConfig` struct; the "opaque handle" comment sits above the
  secret field. The result type is `Receipt` (see tools/overview).
- agents/waits: TS `setTimeout as sleep` maps to
  `golem_rust::wasip3::clocks::monotonic_clock::wait_for as sleep` (ns).
- agents/team: the reflection namespace is `golem_rust::agentic::reflection`;
  tool types expose `lookup_name()` (there is no `name()`).

## Negative controls (neg.sh)

For each line of `neg-cases.tsv`, `neg.sh` writes a mutated copy of one
display file to `neg/` (each from/to pair must occur exactly once; the copy
keeps its markers and build.rs strips them), runs
`cargo check` with `SNIPPET_NEG=<rel>=<copy>` (plus `--features secrets`
for tools/secrets), and requires a non-zero exit;
it fails if any display file has no case, then restores the positive build
and deletes `neg/`.

| Snippet | Planted misuse | Error |
|---|---|---|
| agents/language | drop `#[agent_implementation]` | E0046 missing `agent_implementation_annotation` |
| agents/models | drop `#[derive(Schema)]` from `Review` | E0277 `Review: IntoWire` |
| agents/overview | `approval.json()` without `.await` | E0599 no `json` on `WebhookHandler` |
| agents/sqlite | add a `std::time::Instant` method parameter (trait and impl) | E0599 `AgentArgument<Instant>` bounds |
| agents/team | `a.language()` | E0599 no `language` on `AgentType` |
| agents/waits | `schedule_remind_cancelable` | E0599 on `MailerAgentClient` |
| artifacts/login | `user.subject` | E0609 no field on `OidcPrincipal` |
| artifacts/overview | `write_one(part.len())` | E0308 `AgentStream<usize>` |
| artifacts/readonly | `#[read_only(cache = true)]` | macro: cache must be a string mode |
| artifacts/routers | `.csv()` without `.await` | E0308 expected `AgentStream<Vec<u8>>`, found future |
| artifacts/routes | mount `/reports/{tm}` | macro: filesystem mount capture `{tm}` must bind one constructor parameter |
| artifacts/stream | `AgentStream::channel()` | E0599 |
| tools/custom | `requires_filesystem = "yes"` | macro: must be a bool literal |
| tools/overview | `reveal(&api_key, ..)` | E0308 `&Secret` vs `&GuestSecretHandle` |
| tools/permissions | `Holder::Worker` | E0599 no variant |
| tools/requests | body without `.into_bytes()` | E0308 `Request<Vec<u8>>` |
| tools/secrets (`--features secrets`) | pass `config.stripe_key` (the `Secret<String>` wrapper) instead of `.handle()?` | E0308 expected `GuestSecretHandle`, found `Secret<String>` |

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
- Generated cross-component clients (bash, `ResearchAgent`, `SlackAgent`) are
  stubs, so their calls are checked only against the stub shapes.
- SQLite file persistence under `/data`, webhook delivery, durable
  `wait_for`, scheduled/cancelled reminders, secret reveal, permission-card
  derivation/installation/revocation, file exposure,
  read-only caching, and the router forwarding a remote agent's byte
  stream as its response body are type-checked (and, for HTTP shapes, registration-
  validated) only.
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
- The tool's `api_key: Secret` parameter (`GuestSecretHandle`) is what the
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
