# Go home-page snippets: how they are compiled and checked

The Go ports of the per-feature home-page snippets live in
`website/snippets/<chapter>/<feature>/go/*.go`, one file per feature: 16 in
all (every TypeScript feature except `agents/sqlite`: Go has no embedded
SQLite, so there is no `agents/sqlite/go/` and the page shows the note). Any
shared YAML (e.g. `website/snippets/tools/publish/golem.yaml`) is not Go and
is not checked here.

A line ending in ` //@` is highlighted on the page while its snippet shows;
the page strips the marker, and so does check.sh before every rule and
compile step (the 68-column limit excludes it). Each line marked in the
TypeScript original has a marked Go counterpart: the same Golem operation or
declaration (when it spans several Go lines, the line with the Golem call or
the value the reader should see). A marked TS comment is the same comment,
marked, in Go.

| Chapter / feature | Display file | Agent / tool | Harness package |
|---|---|---|---|
| agents/overview | `support_agent.go` | `SupportAgent` | `pkg/agents-overview` (package `support`) |
| agents/language | `cart_agent.go` | `CartAgent` | `pkg/agents-language` (`cart`) |
| agents/models | `draft_agent.go` | `DraftAgent` | `pkg/agents-models` (`drafts`) |
| agents/team | `planner_agent.go` | `PlannerAgent` | `pkg/agents-team` (`planner`) |
| agents/waits | `checkout_agent.go` | `CheckoutAgent` | `pkg/agents-waits` (`checkout`) |
| tools/overview | `refund_tool.go` | tool `refund` | `pkg/tools-overview` (`refund`) |
| tools/custom | `build_agent.go` | tool `outline`, `BuildAgent` | `pkg/tools-custom` (`build`) |
| tools/requests | `stripe_refund.go` | `StripeRefund` | `pkg/tools-requests` (`stripe`) |
| tools/permissions | `orders_access.go` | `ShareOrders`, `RevokeOrders` | `pkg/tools-permissions` (`access`) |
| tools/secrets | `support_agent.go` | `SupportAgent` (config `stripeKey`) calling tool `refund` | `pkg/tools-secrets` (`secrets`) |
| artifacts/overview | `site_agent.go` | `SiteAgent` | `pkg/artifacts-overview` (`site`) |
| artifacts/routes | `report_agent.go` | `ReportAgent` | `pkg/artifacts-routes` (`reports`) |
| artifacts/routers | `exports_router.go` | router `ExportsRouter` calling `ExportAgent` | `pkg/artifacts-routers` (`exports`) |
| artifacts/stream | `writer_agent.go` | `WriterAgent` | `pkg/artifacts-stream` (`writer`) |
| artifacts/login | `notebook_agent.go` | `NotebookAgent` | `pkg/artifacts-login` (`notebook`) |
| artifacts/readonly | `page_agent.go` | `PageAgent` | `pkg/artifacts-readonly` (`pages`) |

## Re-running

```shell
<repo>/.amp/in/snippets/go/check.sh   # ends with ALL_OK (about 30 s warm)
<repo>/.amp/in/snippets/go/neg.sh     # runs check.sh, then 16 mutations; ends with NEG_OK
```

## Prerequisites (verified 2026-10-21, macOS arm64)

- **Host Go ≥ 1.21** with `GOTOOLCHAIN=auto` (the default). The host had
  go1.26.3; the harness `go.mod` says `go 1.27.1`, so `go` re-execs the stock
  **go1.27.1** from the module cache (downloaded on first use). Go 1.27 is
  required: the SDK uses generic methods (`Agent.Method[In, Out]`,
  `impl.Handle`, `Tool.Body[A, O]`, ...).
- **gofmt 1.27:** the script calls `"$(go env GOROOT)/bin/gofmt"` from the
  harness directory (an older `gofmt` on PATH cannot parse generic methods).
- **The patched Golem Go toolchain `go1.27.1-golem.4`** (github.com/golemcloud/go),
  needed only to link components. check.sh installs it the way the Golem CLI
  does (`cli/golem-cli/src/app/build/go_toolchain.rs`, pins in
  `cli/golem-cli/src/versions.rs`): it downloads
  `go-<os>-<arch>-bootstrap.tbz`, verifies the pinned sha256, unpacks it into
  componentize-go's toolchain directory
  (`~/Library/Caches/componentize-go/v2/go-darwin-arm64-bootstrap` on macOS,
  `$XDG_CACHE_HOME`/`~/.cache` on Linux) and writes the
  `GOLEM_GO_TOOLCHAIN_TAG` marker. `GOLEM_GO_TOOLCHAIN=<dir>` overrides it.
- **componentize-go v0.4.3**, pinned through each component module's `tool`
  directive (as the CLI's Go template does); on first use it downloads its
  binary into `~/Library/Caches/com.github.bytecodealliance-componentize-go`.
- **wasm-tools** (1.247.0 used) for `wasm-tools validate --features all`.
- **python3** (neg.sh string replacement), **perl** (indent conversion),
  curl, shasum, POSIX sh, BSD or GNU sed/awk.
- **Network on the first run:** the toolchains, componentize-go, and modules:
  `github.com/firebase/genkit/go v1.14.0` (pulls
  `github.com/anthropics/anthropic-sdk-go v1.23.0`, `openai-go v1.8.2` and
  others), `github.com/openai/openai-go/v3 v3.84.0`, the SDK's
  `github.com/golemcloud/go-pkg v0.2.3-golem.1` fork and
  `github.com/coder/websocket`. `go.sum` is kept in the harness.
- **sed -E** for marker stripping (`sed -E 's,[ ]+(//|#)@$,,'`), mktemp.
- **No SDK build step:** the in-tree SDK (`sdks/go/golem`, `sdks/go/core`) is
  consumed as source via `replace`; its generated WIT bindings
  (`sdks/go/golem/internal/wit`) are checked in.

## Harness layout

```
<repo>/.amp/in/snippets/go/
  go.mod, go.sum   module example.com/snippets. Requires the SDK, genkit,
                   openai-go/v3 and the three bridge stubs. replace:
                     sdks/go/core, sdks/go/golem => <repo>/sdks/go/...
                     go.bytecodealliance.org/pkg =>
                       github.com/golemcloud/go-pkg v0.2.3-golem.1
                       (mandatory for every Go component; the SDK's own
                       replace does not propagate)
                     golem.local/bridge/<x>-guest-client => ./bridge/<x>-...
                     (research-agent, slack-agent, bash-tool, refund-tool)
  check.sh, neg.sh, PROCESS.md
  bridge/          stand-ins for `golem build`-generated clients (below),
                   each its own module, as the real ones are
  pkg/<chapter>-<feature>/
    prefix.txt     package clause + the imports the display file uses
    companion.go   the omitted supporting code (the TS prelude's analogue)
    definitions_test.go   native golem.DefinitionErrors() check (7 pkgs)
                   (tools-secrets has only prefix.txt: its supporting code
                   is the refund-tool bridge stub)
    snippet.go     GENERATED by check.sh from the display file
  components/, out/   GENERATED and deleted by check.sh
```

What each `companion.go` supplies (the TS prelude counterparts):

- `agents-overview`: `Ticket{Message, OrderID}`, `Approval`, `Reply`,
  `ctx`, the OpenAI client `ai = openai.NewClient()`, `Message`
  (= `openai.ChatCompletionMessageParamUnion`), `replyFormat` (strict JSON
  schema, the analogue of `zodResponseFormat(Reply, 'reply')`), `readReply`
  (rejects no choices, non-`stop` finish, refusals, empty content; strict
  decode), `notifyReviewer`, `refundOrder` (HTTP POSTs).
- `agents-language`: `placeOrder` (POSTs the cart, decodes `{id}`).
- `agents-models`: `ctx` only (`genkit.Init` is in the display file).
- `agents-team`: `PlanResult{Facts, Agents, Tools}` (the TS `Plan` schema;
  renamed because `Plan` is the method descriptor), `inDays`.
- `agents-waits`: `PaymentEvent{Status}`, the same-component `MailerAgent`
  (`Mailer`, `MailerID`, `Remind`, implemented), `inDays`,
  `createPaymentLink`, `sendSurvey`.
- `tools-overview`: `RefundTool` (identity type), `Refund{ID, Status}`,
  `StripeRefund` (a harness copy of the tools/requests snippet).
- `tools-custom`: `Outliner` (identity type), `Args{File}`, `Headings`
  (= `[]string`), `BuildID{Project}`.
- `artifacts-overview`: `streamPage` (`iter.Seq[string]`).
- `artifacts-routes`: `render`, `frontEnd` (both `[]byte`).
- `artifacts-routers`: the same-component `ExportAgent` (`Export`,
  `ExportID{ID}`, method `CSV` = `csv` returning
  `golem.AgentStream[[]byte]` via `ProduceStream`), the TS `Export` /
  `csv()` counterpart. No `definitions_test.go` (see Known limits).
- `artifacts-stream`: `ctx`, `ai = openai.NewClient()`.
- `tools-requests`, `tools-permissions`, `artifacts-login`,
  `artifacts-readonly`: nothing (no companion.go).

## How display files are spliced and checked

`check.sh`:

1. Finds every display file with `find website/snippets -path './*/*/go/*'`
   and reads it in place, writing a marker-stripped copy to a temp dir
   (`sed -E 's,[ ]+(//|#)@$,,'`) that every later step uses. It requires
   exactly one file per `<chapter>/<feature>/go/`, a harness package for each,
   no harness package without a display file, and no `agents/sqlite/go/`.
2. Per display file (awk, string literals blanked first): width ≤ 68, no tabs,
   ≤ 31 lines, every `//` comment on its own line and ending with `:`, no block
   comments, no `package`/`import` lines. Prints `lines=`, `longest=` and
   `markers=` (the count of ` //@` lines). Requires at least one marker, one
   space before it (not more, no tab), and no `//@` left after stripping.
3. Compares the file's comment texts, in order, with the TypeScript original
   (`<chapter>/<feature>/ts/*.ts`, markers stripped); they must be identical
   (Go keeps "a Rust ResearchAgent"). The TS file must have a marker, and the
   marked comments must be the same, in order, in both. (Whether each marked
   TS code line has a marked Go counterpart is a review judgement, not
   automated.)
4. Writes `pkg/<p>/snippet.go = prefix.txt + display text`, with each leading
   run of 4 spaces turned back into a tab
   (`perl -pe '1 while s/^(\t*)    /$1\t/'`). `gofmt -l pkg/*/snippet.go`
   must print nothing: the displayed code is exactly gofmt's output modulo tab
   width (gofmt also demands its blank lines between declarations of
   different kinds).
5. Compiles, printing each command and `rc=`:
   ```
   go version
   GOOS=wasip1 GOARCH=wasm go build ./pkg/<all 16>
   GOOS=wasip1 GOARCH=wasm go vet   ./pkg/<all 16>
   # per snippet, a real Golem component:
   (cd components/<p> && PATH=<golem go>/bin:$PATH \
       go tool componentize-go build -o ../../out/<p>.wasm)
   wasm-tools validate --features all out/<p>.wasm
   go test -count=1 ./pkg/<the 7 with definitions_test.go>
   ```
   `components/<p>/` is generated: a module
   `example.com/snippets/components/<p>` (each Go component must be its own
   module, as in the CLI template) requiring `example.com/snippets`,
   componentize-go v0.4.3 (`tool` directive) and the SDK, with the harness
   replaces re-rooted, and a `main.go` that blank-imports the snippet package
   and the SDK; `go mod tidy` runs with the patched toolchain first.
6. Prints `ALL_OK` only if every step passed.

## Invented libraries, real libraries and stubs

No library had to be invented for Go.

| Import path | Used by | Source and reason |
|---|---|---|
| `github.com/openai/openai-go/v3` v3.84.0 (real) | agents/overview (Chat Completions + strict JSON schema), artifacts/stream (Responses API, `OutputText()`) | The official OpenAI Go SDK, the counterpart of the TS `openai` client; compiles for wasip1 and sends through `net/http`, which the Golem SDK routes via the host. |
| `github.com/firebase/genkit/go` v1.14.0 (real): `genkit`, `ai`, `plugins/compat_oai/openai`, `plugins/anthropic` | agents/models | The Go counterpart of the Vercel AI SDK call: one API over two providers (`genkit.GenerateText` with `openai/gpt-4.1`, `genkit.GenerateData[Review]` typed structured output with `anthropic/claude-sonnet-4-5`). Chosen over langchaingo (its Anthropic provider ignores JSON mode) and over the two official SDKs directly (over 31 lines). The `anthropic` plugin wraps the official `anthropic-sdk-go`. Compiles into a component (~41 MB). |
| `golem.local/bridge/research-agent-guest-client` (stub, `bridge/`) | agents/team | Hand-written stand-in for the client `golem build` generates for a ResearchAgent in another component, shaped after `cli/golem-cli/src/bridge_gen/go/mod.rs`: `DefineFullAgentClient`, `ResearchAgentID{Topic}`, `Facts{Summary, Sources}`, `MustGetResearchAgent`, `MustGather`, `ScheduleRefresh`/`MustScheduleRefresh(at time.Time)` (only the forms used, plus a few). Real SDK calls underneath. |
| `golem.local/bridge/slack-agent-guest-client` (stub) | agents/team | Same, for `SlackAgent{Channel}` with `post(text)`: `MustGetSlackAgent`, `TriggerPost`/`MustTriggerPost`. |
| `golem.local/bridge/refund-tool-guest-client` (stub) | tools/secrets | Stand-in for the client `golem build` generates for the tools/overview `refund` tool, named as `bridge_gen/go/tool.rs` names a tool with a root body: identity type `RefundTool`, `Tool = tool.DefineToolClient[RefundTool]("refund")`, command `Root` with `RootArgs{Charge string, Amount int32, APIKey golem.Secret[string]}` (`to_field_ident("apiKey")` = `APIKey`; a secret parameter is `golem.Secret[inner]`, `bridge_gen/go/type_ref.rs:235`), error `ErrHTTP`, result `Refund{ID, Status}`. Real SDK calls underneath (`Tool.Body`, `Positional`, `Raises`). |
| `golem.local/bridge/bash-tool-guest-client` (stub) | tools/custom | Stand-in for the client generated for the built-in `bash@0.2.1` release, shaped after `bridge_gen/go/tool.rs` and the golem-call-tool-go skill: `tool.DefineToolClient[BashTool]("bash")`, `Run` with `RunArgs{Script, Cwd, Timeout}` (defaults `""`, `600`), result `RunResult{ExitCode, Stdout, Stderr}`. The `Timeout` type (`uint32`) and the result type name are guesses. |

Everything else is the real in-tree SDK: `golem.DefineAgent`/`Spec`/`Mount`
(`Path`, `Auth`, `CORS`, `ExposeFiles`), `Method` with `golem.HTTP(POST/PUT/GET)`
and `golem.ReadOnly()`, `Implement(...).Handle(...)` with `golem.Adapt`/
`Adapt0`/`AdaptUnit`, `golem.NewWebhook[T]`/`URL`/`MustAwait`,
`MustSchedule` + `ScheduledInvocation.Cancel`, durable `time.Sleep`,
`reflection.DiscoverAgentTypes`/`DiscoverTools`, `golem.DefineHTTPRouter`
and `HandleRaw` (`HTTPRequest`/`HTTPResponse`/`HTTPHeader`,
`golem.StreamOf`), `MethodDef.MustCall` returning a stream, `golem.ProduceStream`/`AgentStreamWriter`,
`golem.Principal`/`golem.OidcPrincipal`, `golem.Secret[string].MustGet`,
`golem.DefineConfiguredAgent` + `ctx.Config(def)` (secret config fields),
`tool.DefineTool`/`Spec{Version, RequiresFilesystem}`/`DefineToolError`/
`Body`/`Handle`/`Call`, `golem.PatternGrant`/`Bounds`/`MustDeriveFromWallet`/
`MustDerive`/`LifespanOf`/`Install(AgentHolder)`/`Revoke`.

## Negative controls (neg.sh)

neg.sh runs check.sh, copies each package's `companion.go` and spliced
`snippet.go` to `neg/<p>/` with one planted mistake, requires
`GOOS=wasip1 GOARCH=wasm go build ./neg/<p>/` to fail, then deletes `neg/`.

| Snippet | Mutation | Compiler error |
|---|---|---|
| agents/overview | `approval.MustAwait()` → `approval.Await()` | multiple-value `approval.Await()` in single-value context |
| agents/language | `golem.Adapt0(func(c *cart) string` → `golem.Adapt(...)` | cannot infer S, In, and Out |
| agents/models | `Drafter.Method[WriteIn, Draft]("write")` → `Drafter.Method("write")` | cannot infer In |
| agents/team | `MustScheduleRefresh(inDays(1))` → `MustScheduleRefresh(1)` | cannot use 1 as time.Time |
| agents/waits | `Remind.MustSchedule(mailer, inDays(1), …)` → `Remind.MustTrigger(mailer, …)` | `reminder.Cancel` undefined (golem.InvocationID) |
| tools/overview | `a.APIKey.MustGet()` → `a.APIKey` | cannot use golem.Secret[string] as string |
| tools/secrets | `a.APIKey = ctx.Config(Support).StripeKey` → `….StripeKey.MustGet()` | cannot use string as golem.Secret[string] |
| tools/custom | `golem.Must(Outline.Call(` → `(Outline.Call(` | too many return values |
| tools/requests | `golem.Must(http.NewRequest(` → `golem.Must0(` | too many arguments in call to golem.Must0 |
| tools/permissions | `golem.Some(golem.LifespanOf(time.Hour))` → `golem.Some(time.Hour)` | Option[time.Duration] is not Option[golem.Lifespan] |
| artifacts/overview | `w.Write(part)` → `w.Write(len(part))` | cannot use int as string |
| artifacts/routes | `frontEnd(id.Team), 0o644))` → `frontEnd(id.Team)))` | not enough arguments in call to os.WriteFile |
| artifacts/routers | `rows := CSV.MustCall(export, golem.Unit{})` → `CSV.Call(...)` | assignment mismatch: CSV.Call returns 2 values |
| artifacts/stream | `AgentStream[string]` → `AgentStream[int]` in `Method` | handler type does not match |
| artifacts/login | `Auth: true` → `Auth: "pkce"` | cannot use string as bool |
| artifacts/readonly | `golem.ReadOnly()` → `golem.ReadOnly(true)` | cannot use true as golem.CachePolicy |

## The Secrets snippet (tools/secrets)

`SupportConfig{StripeKey golem.Secret[string]}` is attached with
`golem.DefineConfiguredAgent[SupportID, SupportConfig]` (a `Secret`-typed field
of the config struct becomes a secret config key `stripeKey`;
`sdks/go/golem/config.go`). The handler takes the raw
`func(ctx *golem.Context[support], in RefundIn) refund.Refund` form so it can
read `ctx.Config(Support).StripeKey` and assign it to the generated client's
`RootArgs.APIKey` (also `golem.Secret[string]`) in
`refund.Root.Call(func(a *refund.RootArgs) {...})`. No shim is needed.

The Go SDK does not have the TS bug (read from source, not run): config and
tool parameters share one type, `golem.Secret[T]`, so a revealed string
cannot be passed (the neg control proves it), and the shared codec
(`sdks/go/golem/codec_sdk.go:68-99`, used by tool calls through
`tool/definitions.go`'s `link.Engine` and `tool/tooldef.go:1074` `encode`)
encodes a secret by `secretTake()`. For a config secret, `take` mints a fresh
host handle with `get-config-value` (`secret_wasm.go:55-65`,
`configSecretHandle`), never the plaintext; a received secret is adopted as a
handle and moved on send. The Go tool bridge generator types secret
parameters as `golem.Secret[inner]` (`cli/golem-cli/src/bridge_gen/go/type_ref.rs:235-238`).
Not verified at runtime: nothing was deployed, and there is no SDK test that
sends a config secret through a tool call.

The `SupportAgent` definitions pass `golem.DefinitionErrors()` natively in a
one-off test with the handler body stubbed (the full package does not link
natively: `ctx.Config` reaches `get-config-value`), so there is no
`definitions_test.go` for it.

## Departures from the TypeScript (by design)

- **Omitted declarations.** As in TS, schemas and helpers live in the
  harness. Go also hides the tool identity types (`RefundTool`, `Outliner`),
  and, for tools/custom only, `Args{File}` and `BuildID{Project}` (needed to
  stay within 31 lines).
- **Handler shape.** Single-method agents chain
  `Implement(init).Handle(Method, golem.Adapt…(func(state, in) …))`; the
  state is the agent's ID struct where TS keeps only the id's fields, and an
  empty struct where TS returns `{}`.
- **Exported names.** TS `export`ed functions are exported in Go
  (`StripeRefund`, `ShareOrders`, `RevokeOrders`); `ordersAPI` stays
  unexported. `Plan`'s result type is `PlanResult`.
- **artifacts/stream** puts its comment inside the `Method(...)` call, above
  the route argument (TS: above `returns:` inside `method({...})`), because a
  decl-level comment would force a gofmt blank line and a 32nd line.
- **artifacts/readonly** puts its three comments inside the `read`
  `Method(...)` call, above `golem.ReadOnly()`, as TS does above `readOnly`.
- **artifacts/routes** writes the front-end in the `Implement` constructor
  (TS `init`); `os.WriteFile("/site/index.html", frontEnd(id.Team), …)`
  spans two lines and the marker is on the `frontEnd(...)` line.
- **artifacts/routers** declares the path regexp as a package-level
  `var csvPath` (TS uses an inline literal; inline it is over 68 columns),
  marks the `FindStringSubmatch` line for the TS `match` line, and calls the
  agent in two lines (`export := Export.Get(...)`, then the marked
  `rows := CSV.MustCall(export, golem.Unit{})`). The 404 omits `Headers`
  (nil encodes as an empty list) and uses `golem.StreamOf[[]byte]()` for
  TS `empty()`; the method is named `CSV` (Go initialism) for the TS `csv`.
  The returned stream is handed on as the response body unread: a pure
  endpoint move (`sdks/go/golem/codec_stream.go:36-52, 82-85`), so no SDK
  gap and no shim.
- **artifacts/routes** declares the ReportAgent mount as `var mount` so its
  comment fits at a 4-space indent (inside `Spec{HTTP: &Mount{...}}` it would
  be 70 columns).
- **artifacts/overview** opens the stream closure as `ProduceStream(func(`
  with the parameter on the next line, so its comment fits in 68 columns.
- **tools/overview** declares the handler as a top-level `func refund`
  (registered with `.Handle(refund)` chained on `Tool.Body`), with "each call
  gets a fresh VM that can't read the agent's memory:" above it: inside
  `Handle(` the comments would exceed 68 columns. **tools/custom** keeps
  "each call runs in a fresh WebAssembly VM:" inside `Handle(`.
- **tools/secrets** uses the generated client's `Root`/`RootArgs` names (a
  tool's root command), where TS writes `refund.client().refund({...})`.
- **tools/requests** marks the URL line and the `Do` line (TS marks the one
  `fetch` line).
- **tools/custom** filters headings with `regexp` `(?m)^#.*` (lines starting
  with `#`, the same set as TS `line.startsWith('#')`).
- **agents/models** shows the providers in `genkit.Init(...WithPlugins(...))`
  instead of import lines; structured output is `GenerateData[Review]`.
- **Errors.** TS `throw` becomes `panic` (traps the invocation, like an
  uncaught throw); TS `.unwrap()` becomes `golem.Must`/`Must0`.

## Known limits

- Components are built and validated, but nothing was deployed or run on a
  Golem server.
- `golem.DefinitionErrors()` (mount captures, file mappings, endpoint
  options, schema-able types) runs natively for the 7 packages that link
  without host imports (artifacts-login, -overview, -readonly, -routes,
  tools-custom, -permissions, -requests). The other 9 reach `net/http` (the
  SDK's host-backed transport) or RPC and fail to link natively
  (`relocation target wasm_import_… not defined`); their definitions were
  checked once with an equivalent definitions-only native test (all clean,
  and a wrong `{order}` capture was reported), not on every run. For
  artifacts-routers (2026-10-22) the `Export.Get`/`MustCall` lines were
  stubbed out: clean, and a mount of `/exports/{x}` was reported ("router
  mount must be made of literal segments").
- genkit's runtime behaviour inside a Golem component (its `Init`, the
  Anthropic structured-output path) was not exercised; only compilation.
- check.sh and neg.sh share the generated `pkg/*/snippet.go`, `components/`
  and `out/`: do not run two of them at once.
- The bridge stubs approximate generated code; regenerate against real
  callee schemas before relying on exact names or types.
