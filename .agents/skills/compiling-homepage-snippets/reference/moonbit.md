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
bash .amp/in/snippets/moonbit/neg.sh     # 20 negative controls + a clean run; ends with NEG_ALL_OK
```

A warm `check.sh` run takes about 20 s (40 code-generator runs, one
`moon build`, 20 component links); `neg.sh` runs `check.sh` 21 times, about
10 minutes. The first run is longer while Moon compiles the SDK and the
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
| `artifacts/overview/moonbit/analyst_agent.mbt` | `harness/artifacts_overview` |
| `artifacts/routes/moonbit/app_tool.mbt` | `harness/artifacts_routes` |
| `artifacts/files/moonbit/chart_agent.mbt` | `harness/artifacts_files` |
| `artifacts/login/moonbit/private_charts_agent.mbt` | `harness/artifacts_login` |
| `tools/middleware/moonbit/refund_policy.mbt` | `harness/tools_middleware` |
| `artifacts/peruser/moonbit/dashboard_agent.mbt` | `harness/artifacts_peruser` |
| `artifacts/routers/moonbit/exports_router.mbt` | `harness/artifacts_routers` |
| `artifacts/signin/moonbit/billing_agent.mbt` | `harness/artifacts_signin` |

There is no `agents/sqlite/moonbit` (owner's decision: MoonBit has no
embedded SQLite; the page shows a note). `check.sh` fails if that folder
appears. So the page's 21 features (7 per chapter) have 20 MoonBit display files.
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
| `artifacts_overview` | application helpers `is_quarter(q)` (the `^Q[1-4]-\d{4}$` rule of the TS `Quarter` schema), `chart_png(q) -> FixedArray[Byte]`, `page_for(q)`, and **`run_model_loop(question, tool, max_steps~, tool_description~)`**: a real OpenAI Chat Completions tool-calling loop over the SDK's WASI HTTP client (`@http.send`, `json_post` with an `Authorization` header, `@json.parse`; key from the `OPENAI_API_KEY` environment variable via `wasi:cli/environment`) that offers the model one function tool `chart(quarter)` (JSON-schema `pattern` on `quarter`), calls the displayed closure for each tool call, appends `role: tool` results and returns the final text. It stands in for the Vercel AI SDK's `generateText` + `tool` + `stepCountIs` because MoonBit has no model library with tool calling (see "Model library" below) |
| `artifacts_routes` | `#derive.golem_schema pub(all) struct Row { week, total : Double }`, `query_sales(region)` |
| `artifacts_files` | `SaleRow`, `query_sales(team, quarter)`, `draw_chart(rows) -> Bytes` |
| `artifacts_login` | `Chart { asked_by, png : Bytes }`, `answer_with_chart(question) -> Bytes`, `user_key(caller) -> String raise` (OIDC `[issuer, sub]` as JSON; fails for non-OIDC callers, as the TS prelude's `userKey`) |
| `tools_middleware` | the Secrets `refund` tool (same name, version, positional `charge`, `amount`, `api_key : @secrets.SecretHandle[String]`, error `RefundError::Http`) with `customer_email` in `RefundResult` and a host-injected `principal : @tool.Principal` parameter (see "SDK gap: caller principal in typed middleware"); `check_rate_limit(principal) -> Result[Unit, @toolMiddleware.ToolInvokeError[E]]` (a constraint violation over 10 calls per principal, as the TS prelude's `checkRateLimit`), `ask_finance(approval, charge, amount)`, `approved(verdict : Bytes)` (`{"approved": true}`), `audit(charge, amount, refund_id)`. The middleware must be in the tool's package (`#derive.tool_middleware` resolves `presented=` same-package only, `golem_sdk_tools/lib/tool_middleware.mbt:308-331`) |
| `artifacts_peruser` | `dashboard_for(user, request) -> String` (the model loop that builds the dashboard) |
| `artifacts_routers` | the user's `#derive.agent struct AnalystAgent(chat)` with `export_csv(id) -> @schema.AgentStream[Array[Byte]]` (so the generator emits `AnalystAgentClient` in this component; the TS prelude declares it the same way, "defined elsewhere in the app"), `is_word` (the `[\w-]+` class), `header(name, value) -> @http.HttpHeader`, `empty()` (the TS prelude's `header`/`empty`) |
| `artifacts_signin` | `Invoice`, `invoices_of(org, issuer, sub)`, `page_for(question, invoices)` |
| `tools_secrets` | a `refund` tool (same signature as the Requests harness, trivial body) and `RefundResult`, so the generator emits the typed tool client `RefundClient` in this component |
| `agents_stream`, `agents_readonly` | nothing |

`util` (`golemcloud/home_snippets_v3/util`): `json_post` (optional extra
`headers`), `form_post`
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
| analyst_agent | `expose_files("/*", "/out/$1")` → `expose_files("/*", "/out")` | golem_sdk_tools: `AgentParseError` (a `/*` subtree needs a `/$1` target) |
| app_tool | `get="/sales?region={region}"` → `get="/sales?region={area}"` | golem_sdk_tools: `AgentParseError` (query variable is not a parameter) |
| chart_agent | `UnstructuredBinary[Png]` → `UnstructuredBinary[String]` | moonc: `String does not implement trait AllowedMimeTypes` |
| private_charts_agent | `caller : @common.Principal` → `caller : String` (both methods) | moonc: type mismatch (`String` vs `@common.Principal` at `user_key(caller)`) |
| digest_agent | `SummarizeClient::new().summarize(` → `Summarize::client().summarize(` | moonc: `Summarize has no method client` |
| build_agent | `OutlineClient::new().outline(` → `Outline::client().outline(` | moonc: `Outline has no method client` |
| stripe_refund | drop `.unwrap()` after `@http.send(...)` | moonc: type mismatch (`Result` vs `@http.Response`) |
| orders_access | `lifespan=@permissions.Lifespan::iso8601("PT1H")` → `lifespan="PT1H"` | moonc: type mismatch |
| account_agent | drop `.unwrap()` after `.crm_find_customer(email)` | moonc: `Result[@tool.TypedToolInvocation[…], @tool.ToolError[…]] has no method collect` |
| refund_policy (middleware) | `@api.await_promise(approval)` → `@api.get_promise(approval)` | moonc: type mismatch (`@api.PromiseResult` vs `Bytes` at `approved(verdict)`) |
| dashboard_agent | `fn DashboardAgent::new(user : String)` → `(name : String)` | golem_sdk_tools: `AgentParseError` (mount variable `{user}` is not a constructor parameter) |
| exports_router | `AnalystAgentClient::get(` → `AnalystAgent::get(` | moonc: `Type AnalystAgent has no method get` |
| billing_agent | `caller : @common.Principal` → `caller : String` | moonc: `The value identifier Oidc is unbound` |
| support_agent (secrets) | pass `self.config.stripe_key` (a `@config.Secret[String]`) straight to `RefundClient::refund` | moonc: type mismatch (`@config.Secret[String]` vs `@secrets.SecretHandle[String]`) |

The code generator prints only the error type (`AgentParseError`), not its
message, so those six controls are identified by the failing
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
- Tool middleware (`tools/middleware`): `#derive.tool_middleware("refund-policy",
  presented="Refund")` on an empty struct (parsed at
  `golem_sdk_tools/lib/tool_middleware.mbt:91,147`; documented in
  `docs/src/content/v1.6/how-to-guides/moonbit/golem-tools-middleware-moonbit.mdx`;
  SDK sample `golem_sdk/tool-middleware-authoring-test/authoring.mbt:76,136-148`),
  with a `pub async fn RefundPolicy::refund(underlying : RefundUnderlying,
  <the tool's parameters>, principal : @tool.Principal)` handler whose
  parameters and return type `Result[RefundResult,
  @toolMiddleware.ToolInvokeError[RefundError]]` the generator checks
  (`tool_middleware.mbt:742-797`). The generated
  `RefundUnderlying::refund(charge, amount, api_key)` returns the same
  `Result` (`harness/tools_middleware/golem_tool_middlewares.mbt:283`), and
  the generated dispatcher passes `__golem_invocation.principal()` to the
  handler (same file, line 475). Rejections are
  `@toolMiddleware.ToolInvokeError::ConstraintViolation(String)`
  (`golem_sdk/tool-middleware/runtime.mbt:2`), the TS
  `ToolInvokeError({ tag: 'constraint-violation' })`.
- Promises (`tools/middleware`): `@api.create_promise() -> @api.PromiseId`
  (`golem_sdk/api/promises.mbt:13`) and `@api.await_promise(id) -> Bytes`
  (`promises.mbt:51`, "the agent is durably suspended … until the promise
  is fulfilled"), as in `docs/src/content/v1.6/develop/promises.mdx`
  (MoonBit tabs).
- HTTP router (`artifacts/routers`): `#derive.http_router` +
  `#derive.mount("/exports")` on an empty struct with `fn new()`, and a
  `#derive.http_handler pub async fn serve(_self, request :
  @http.HttpRequest) -> @http.HttpResponse`
  (`golem_sdk_tools/lib/agents.mbt:1295,1469`; `HttpHeader`, `HttpRequest`,
  `HttpResponse` at `golem_sdk/http/handler.mbt:3,11,24`; body
  `@schema.AgentStream[Array[Byte]]`), as in
  `docs/src/content/v1.6/invoke/making-custom-apis/http-routers.mdx` and
  `golem_sdk_example1/golem_moonbit_examples/http_router.mbt:3-50`. The
  router calls the agent through the generated client
  `AnalystAgentClient::get(chat)` / `export_csv(id) ->
  @schema.AgentStream[Array[Byte]]`
  (`harness/artifacts_routers/golem_clients.mbt:11,97`), the router-to-agent
  form in the same doc ("Make the handler `pub async fn` before awaiting
  the generated client"), and returns the stream as the response body.
- Per-user agents (`artifacts/peruser`): the mount variable `{user}` is the
  constructor parameter (the generator rejects a mount variable that is
  not one); `#derive.mount_auth(true)` as in `signin`;
  `#derive.expose_files("/*", "/out/$1")` as below; each dashboard file
  is named by a fresh UUID from `@api.generate_idempotency_key_string()`
  (`golem_sdk/api/idempotency.mbt:11`, a host-generated UUID that is
  persisted and never replayed, as `@types.uuid_to_string`; the MoonBit
  tab of `docs/src/content/v1.6/develop/additional.mdx:48` shows
  `@api.generate_idempotency_key()`). `harness/artifacts_peruser/moon.pkg.in`
  imports `golemcloud/golem_sdk/api` for it.
- Sign-in (`artifacts/signin`): `#derive.mount_auth(true)`
  (`golem_sdk_tools/lib/agents.mbt:798`) and a host-injected `caller :
  @common.Principal` parameter; `guard caller is Oidc(oidc)` then `let {
  issuer, sub, .. } = oidc` (`OidcPrincipal`,
  `golem_sdk/interface/golem/agent/common/top.mbt:207,220`).
- Artifacts (all four older ones are made on request, nothing is pre-built):
  `#derive.expose_files("/*", "/out/$1")` on a mounted agent (live-file
  binding, generated as `@common.FileMapping::Subtree({ public_prefix: [],
  filesystem_root: "/out" })`; the generator requires the `/$1` target for
  a `/*` route, `golem_sdk_tools/lib/http_files.mbt:89,112-114`);
  `@fs.get_root_dir`, `Descriptor::create_directory_at`, `@fs.write_bytes`
  / `@fs.write_string` (`golem_sdk/filesystem/filesystem.mbt:192,234`);
  a Golem tool `#derive.tool("present", requires_filesystem=true)` on an
  empty struct with `#derive.arg("name", regex="^[a-z0-9-]{1,40}$")` (a
  real argument refinement: generated `@tool.refine_text(…,
  Some("^[a-z0-9-]{1,40}$"), …)`; same attribute in
  `golem_sdk_example1/golem_moonbit_examples/canonical_tools.mbt:272`,
  parsed at `golem_sdk_tools/lib/tools.mbt:676`) and a plain `String`
  result; `#derive.endpoint(get="/sales?region={region}")` and
  `get="/chart.png?quarter={quarter}"` (generated `query_vars`);
  `#derive.endpoint(post="/ask")`, `get="/charts/{n}"`; a `///` doc
  comment above `#derive.endpoint` becomes the method description
  (generated `description: "Draw a sales chart for one quarter"`), which an
  MCP deployment exposes as the tool description; an unstructured binary
  result `@schema.UnstructuredBinary[Png]` (optional `…[Png]?` in `login`,
  `None` → 404) with a phantom `Png : @schema.AllowedMimeTypes`
  (`["image/png"]`, the MoonBit form of `s.unstructuredBinary({ mimeTypes
  })`, `golem_sdk/schema/unstructured.mbt`) returned as `Inline(bytes~,
  mime_type=Some("image/png"))`; `#derive.mount_auth(true)` (generated
  `auth_details: Some({ required: true })`); `caller :
  @common.Principal` parameters (host-injected; `Oidc({ issuer, sub, .. })`,
  `golem_sdk/interface/golem/agent/common/top.mbt:207`).

## Model library for `artifacts/overview`

MoonBit has no model library with tool calling. The Golem docs say so and
prescribe calling the provider's REST API over WASI HTTP:
`docs/src/content/v1.6/how-to-guides/moonbit/golem-add-llm-moonbit.mdx:5`
("There are no AI-specific libraries for MoonBit … call the provider's REST
API directly using WASI HTTP") and
`docs/src/content/v1.6/develop/ai.mdx:8-13` (the `golem-ai-llm` crates are
Rust-only; MoonBit agents call provider HTTP APIs directly). The SDK has no
`golem:llm` binding (`sdks/moonbit/golem_sdk/wit/main.wit` imports none).
So, as the brief allows, the display keeps the tool body visible and passes
it to the labelled application helper `run_model_loop` (harness), which is
implemented for real: OpenAI Chat Completions function calling
(`tools: [{ type: "function", function: { name, description, parameters
} }]`, `choices[0].message.tool_calls[].function.arguments`, `role: "tool"`
replies with `tool_call_id`) over the SDK's `@http.send`
(`golem_sdk/http/http.mbt:39`). It compiles and links; it was not run
against OpenAI. The invented `@llm` library below is not used here.

## Invented libraries

| Import path (alias) | Used as | Why | Harness implementation |
|---|---|---|---|
| `golemcloud/golem_ai/llm` (`@llm`), used by `agents/models` and `agents/stream` only (no Artifacts snippet uses it) | `@llm.openai(name)`, `@llm.anthropic(name)`, `@llm.generate_text(model=, prompt=)`, `@llm.generate_object(model=, prompt=) -> T`, `@llm.chat_object(messages, model=) -> T`, `@llm.Message` | No MoonBit LLM library exists (the moonbit LLM skill says to hand-roll HTTP). Golem publishes the provider-neutral `golem:llm` interface and the Rust crates `golem-ai-llm`, `-openai`, `-anthropic`; this is their plausible MoonBit binding with the Vercel AI SDK surface the TS snippets use (`openai(...)`, `anthropic(...)`, `generateText`, `Output.object`) and OpenAI's `chat.completions.parse`. Structured output is typed by any `T : @schema.IntoSchema + FromJson`, i.e. a `#derive.golem_schema` record. Reworked from the previous harness's `send`/`json_schema`. | Type-correct; every call raises `LlmError` at runtime. |
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

## SDK gap: caller principal in typed middleware (Middleware snippet)

TS gives every typed middleware handler the caller in its context
(`refund: async (args, { underlying, principal })`). MoonBit typed
middleware handlers receive the caller principal only when the presented
tool itself declares a host-injected `principal : @tool.Principal`
parameter.

- Where: `sdks/moonbit/golem_sdk_tools/lib/tool_middleware.mbt:366-390`
  (`projected_middleware_param`: a `Hidden(Principal)` tool parameter
  becomes the handler's `@tool.Principal` parameter; nothing else adds
  one) and `:1110` (`middleware_surfaces(presented, …, true)`), checked by
  `validate_handler_signature` (`:750-797`, exact parameter list).
- What fails: with the Secrets `refund` tool exactly as it is (no
  principal parameter), a handler `RefundPolicy::refund(underlying, charge,
  amount, api_key, principal : @tool.Principal)` makes both generator
  commands fail with `AgentParseError` (tried by temporarily removing the
  parameter from `harness/tools_middleware/harness.mbt`; restored).
- Workaround used: the harness's `refund` tool declares `principal :
  @tool.Principal` (a real SDK feature; the host injects it, so callers and
  the generated `RefundClient` still pass only `charge, amount, api_key`).
  The display file is the natural code.
- Fix: always offer the invocation principal to typed handlers (the
  generated dispatcher already has it: `__golem_invocation.principal()`,
  `golem_sdk/tool-middleware/raw.mbt:440`), e.g. as an optional trailing
  `principal : @tool.Principal` handler parameter.

## Tools gap: `lexmatch` with captures is rejected by the code generator (found with the Routers snippet)

`artifacts/routers` therefore matches the path with string patterns
(`request.path is [.. "/exports/", .. rest, .. ".csv"]`, a split into
`[chat, id]`, `is_word` and `id.length() == 36`) instead of the TS regex;
`artifacts/overview` validates the quarter with the harness helper
`is_quarter` for the same reason.

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
  `tools/publish`) are used inline without `drop()`,
  mirroring the TS one-liners.
- `agents/overview`: TS `id: { inbox }` is the constructor parameter, so
  the identity comment is above `fn TriageAgent::new(inbox : String)`
  (not highlighted, as the TS `id` line). `z.record(z.string(), Label)` is `Map[String, Label]`.
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
- `artifacts/overview`: same structure, comments and highlights as TS
  (`expose_files` line, the tool declaration line, the two writes). The TS
  `tool({ description, inputSchema: Quarter, execute })` is a MoonBit
  closure `let chart = async fn(q : String) -> String { … }` whose body is
  the TS `execute` body (`mkdir`, the PNG write, the HTML write, the page
  path `/chats/{chat}/<q>.html`). TS validates the quarter with the
  off-screen `Quarter` schema; MoonBit shows one extra line, `guard
  is_quarter(q) else { fail(…) }`, before the quarter reaches a path.
  `generateText({ model, prompt, tools: { chart }, stopWhen:
  stepCountIs(5) })` is the labelled application helper
  `run_model_loop(question, chart, max_steps=5, tool_description=…)` in the
  harness (a real OpenAI tool-calling loop over WASI HTTP, see the harness
  table); the tool description string is the TS one. The model name
  (`gpt-4.1`) is inside the helper, as `model` is in the TS prelude.
- `artifacts/routes`: `toolDefinition('present', { requiresFilesystem:
  true })` is `#derive.tool("present", requires_filesystem=true)` (not
  highlighted, as in TS). `b.positional('name', Name)` with the prelude's
  `/^[a-z0-9-]{1,40}$/` is `#derive.arg("name", regex=…)` on the method
  (the regex is visible here, off-screen in TS; the arguments keep the
  default scope, as in `tools/custom`). `ok(\`${name}.html\`)` is the plain
  `String` result. `init: () => ({})` is the identity constructor
  `AppAgent::new(chat)`; the struct is written on one line to fit. TS
  declares and implements `sales` separately; MoonBit has one function,
  so the TS comment and highlight on the implementation are on its body
  line `query_sales(region)` (the endpoint line keeps its own). The blank
  line between the tool and the agent is gone, as in TS.
- `artifacts/files`: the TS `description:` is the `///` doc comment, placed
  under the TS comment and above `#derive.endpoint`; the TS comment above
  `returns: Png` sits above the `) -> @schema.UnstructuredBinary[Png]`
  line (as in `agents/stream`). `const Png = s.unstructuredBinary(...)` is
  the 4-line phantom `Png` + `AllowedMimeTypes` impl. `{ tag: 'inline',
  val, mimeType }` is `Inline(bytes~, mime_type=Some("image/png"))`.
- `artifacts/login`: `http.mount(..., { auth: true })` is
  `#derive.mount("/private/{team}")` + `#derive.mount_auth(true)` (no
  comment or highlight, as in TS; sign-in moved to `signin`). The TS
  comment above the owner check sits above the highlighted owner guard. `charts: Chart[]` is a field
  initialised in `new`; the struct is on one line, and the `ask`/`open`
  signatures wrap after the first parameters, to fit 31 lines.
  `this.charts.length` is `self.charts.length()` (1-based number, as TS).
  `this.charts[n - 1]` + `made?.askedBy !== userKey(caller)` is two guards
  (`get(n - 1) is Some(made)` and the highlighted owner check); `return
  undefined` is `return None` (404).
- `tools/publish`: the three-line comment above the client call is the TS
  one, verbatim.
- `tools/middleware`: TS `refund.middleware({ name: 'refund-policy',
  implementation: { refund } })` is `#derive.tool_middleware("refund-policy",
  presented="Refund")` + `struct RefundPolicy {}` + the `refund` handler
  (highlight on the attribute). `refuse` is a 3-line generic function
  returning `ConstraintViolation(val)`; every TS `throw refuse(...)` is
  `return Err(refuse(...))`. TS `args` are the positional parameters
  (`args.amount` → `amount`); `principal` is the handler parameter (see the
  gap above). `await checkRateLimit(principal)` (throws a constraint
  violation) is `if check_rate_limit(principal) is Err(e) { return Err(e)
  }`. `createPromise()` / `awaitPromise(approval)` are
  `@api.create_promise()` / `@api.await_promise(approval)`. `await
  underlying.refund(args)` (throw propagates) is `let forwarded =
  underlying.refund(…)` + `guard forwarded is Ok(receipt) else { return
  forwarded }` (the unhighlighted TS line becomes two unhighlighted lines).
  `{ ...receipt, customerEmail: '[redacted]' }` is `Ok({ ..receipt,
  customer_email: "[redacted]" })`. The handler signature takes 5 lines
  (parameters and a wrapped return type) to stay within 68 characters.
- `artifacts/peruser`: `http.mount('/dashboards/{user}', { auth: true,
  exposeFiles })` is `#derive.mount(...)` (highlighted, its comment
  above), `#derive.mount_auth(true)` (no comment or highlight, as in TS)
  and `#derive.expose_files("/*", "/out/$1")`. `init: ({ id }) => ({ user:
  id.user })` is the highlighted constructor `fn DashboardAgent::new(user)`
  with its comment above. `const name = crypto.randomUUID()` is `let name
  = @api.generate_idempotency_key_string()` (no comment, as in TS); the
  file is `/out/<name>.html` and the result
  `/dashboards/<user>/<name>.html`. The write wraps over two lines.
- `artifacts/routers`: `defineHttpRouter('ExportsRouter').mount('/exports')
  .implementRaw(...)` is `#derive.http_router` + `#derive.mount` + struct +
  `fn new()` + `#derive.http_handler pub async fn serve`. The regex match
  (see the lexmatch gap) is one `guard`: highlighted lines are the method
  check (`if (request.method !== 'GET' …)`), the path pattern
  (`request.path.match(path)`) and the 404 `return`; the split/length lines
  stand for the unhighlighted regex line. `empty()` and `header()` are
  off-screen helpers, as in the TS prelude. `Analyst.client.get({ chat })
  .exportCsv({ id })` is `AnalystAgentClient::get(chat.to_string())
  .export_csv(id.to_string())` (highlight on the first line, as TS).
- `artifacts/signin`: `{ auth: true }` is `#derive.mount_auth(true)`
  (highlighted, comment above); `caller: s.principal()` is the highlighted
  `caller : @common.Principal` parameter with its comment above. `if
  (caller.tag !== 'oidc') throw` is `guard caller is Oidc(oidc) else {
  fail("sign-in required") }`; `const { issuer, sub } = caller` is the
  highlighted `let { issuer, sub, .. } = oidc`.
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
  model tool loop of `artifacts/overview`, which needs an OpenAI key, the
  served `/out` files, the PNG responses and the 404 for a different
  user). The `@llm` model calls raise by design and the bash stub aborts. Webhook suspension, durable
  sleep, scheduled/cancelled calls, reflection results, permission-card
  derivation/installation/revocation, secret reveal, file exposure, auth, durable streams, read-only caching, MCP description
  exposure, imported MCP tool calls, middleware interception, promise
  suspension and router responses were not observed on a Golem server.
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

## artifacts/overview: UUID file names (Oct 22 2026)

The chart tool names the PNG and the HTML page with one fresh UUID, as artifacts/peruser does, and `pageFor(q, name)` points the page's `<img>` at `<name>.png`. `let name = @api.generate_idempotency_key_string()`; `harness/artifacts_overview/moon.pkg.in` imports `golemcloud/golem_sdk/api`. The HTML write is wrapped onto two highlighted lines to fit 68 columns. No comment explains the UUID (owner ruling). Re-verified: `check.sh` ALL_OK, `neg.sh` passing.
