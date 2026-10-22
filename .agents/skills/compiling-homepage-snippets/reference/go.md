# Go home-page snippets: how they are compiled and checked

The Go ports of the per-feature home-page snippets live in
`website/snippets/<chapter>/<feature>/go/*.go`, one file per feature: 16 in
all. The home page has 17 features (`website/src/data/homepage.ts`:
`agentsChapter`, `toolsChapter`, `artifactsChapter`); every one has a Go port
except `agents/sqlite` (Go has no embedded SQLite, so there is no
`agents/sqlite/go/` and the page shows the note). check.sh holds the list of
17 and fails if a feature has no TypeScript original, a Go feature has no Go
file, or a Go directory is not a home-page feature.

The TypeScript files in `website/snippets/*/*/ts/` are the reference. A line
ending in ` //@` is highlighted on the page while its snippet shows; the page
strips the marker, and so does check.sh before every rule and compile step
(the 68-column limit excludes it). Each line marked in the TypeScript original
has a marked Go counterpart: the same Golem operation or declaration (when it
spans several Go lines, the line with the Golem call or the value the reader
should see, or all of them when TS marks every line of the statement). A
marked TS comment is the same comment, marked, in Go.

| Chapter / feature | Display file | Agent / tool | Harness package |
|---|---|---|---|
| agents/overview | `triage_agent.go` | `TriageAgent` | `pkg/agents-overview` (package `triage`) |
| agents/models | `draft_agent.go` | `DraftAgent` | `pkg/agents-models` (`drafts`) |
| agents/team | `planner_agent.go` | `PlannerAgent` | `pkg/agents-team` (`planner`) |
| agents/waits | `checkout_agent.go` | `CheckoutAgent` | `pkg/agents-waits` (`checkout`) |
| agents/stream | `writer_agent.go` | `WriterAgent` | `pkg/agents-stream` (`writer`) |
| agents/readonly | `stock_agent.go` | `StockAgent` | `pkg/agents-readonly` (`stock`) |
| tools/overview | `digest_agent.go` | tool `summarize`, `DigestAgent` | `pkg/tools-overview` (`digest`) |
| tools/custom | `build_agent.go` | tool `outline`, `BuildAgent` | `pkg/tools-custom` (`build`) |
| tools/requests | `stripe_refund.go` | `StripeRefund` | `pkg/tools-requests` (`stripe`) |
| tools/permissions | `orders_access.go` | `ShareOrders`, `RevokeOrders` | `pkg/tools-permissions` (`access`) |
| tools/secrets | `support_agent.go` | `SupportAgent` (config `stripeKey`) calling tool `refund` | `pkg/tools-secrets` (`secrets`) |
| tools/publish | `account_agent.go` | `AccountAgent` calling MCP tool `crm_find_customer` | `pkg/tools-publish` (`account`) |
| artifacts/overview | `analyst_agent.go` | `AnalystAgent` with a Genkit `chart` tool | `pkg/artifacts-overview` (`analyst`) |
| artifacts/routes | `app_tool.go` | Golem tool `present`, `AppAgent` | `pkg/artifacts-routes` (`apps`) |
| artifacts/files | `chart_agent.go` | `ChartAgent` | `pkg/artifacts-files` (`chartfiles`) |
| artifacts/login | `private_chart_agent.go` | `PrivateChartsAgent` | `pkg/artifacts-login` (`private`) |

## Re-running

```shell
<repo>/.amp/in/snippets/go/check.sh   # ends with ALL_OK (about 30 s warm)
<repo>/.amp/in/snippets/go/neg.sh     # runs check.sh, then 16 mutations; ends with NEG_OK
```

## Prerequisites (verified 2026-10-22, macOS arm64)

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
                   openai-go/v3 and the five bridge stubs. replace:
                     sdks/go/core, sdks/go/golem => <repo>/sdks/go/...
                     go.bytecodealliance.org/pkg =>
                       github.com/golemcloud/go-pkg v0.2.3-golem.1
                       (mandatory for every Go component; the SDK's own
                       replace does not propagate)
                     golem.local/bridge/<x>-guest-client => ./bridge/<x>-...
                     (research-agent, slack-agent, bash-tool, refund-tool,
                     crm-find-customer-tool)
  check.sh, neg.sh, PROCESS.md
  bridge/          stand-ins for `golem build`-generated clients (below),
                   each its own module, as the real ones are
  pkg/<chapter>-<feature>/
    prefix.txt     package clause + the imports the display file uses
    companion.go   the omitted supporting code (the TS prelude's analogue)
    definitions_test.go   native golem.DefinitionErrors() check (8 pkgs)
    snippet.go     GENERATED by check.sh from the display file
  components/, out/   GENERATED and deleted by check.sh
```

What each `companion.go` supplies (the TS prelude counterparts):

- `agents-overview`: the `Label` enum (`uint8`, `golem.DefineEnum` with
  `urgent`, `billing`, `other`; the TS `Label` z.enum), `parseLabel` (rejects
  anything else, as `Label.parse`), `classify` and `applyLabel` (HTTP POSTs).
- `agents-models`: `ctx` only (`genkit.Init` is in the display file).
- `agents-team`: `PlanResult{Facts, Agents, Tools}` (the TS `Plan` schema;
  renamed because `Plan` is the method descriptor).
- `agents-waits`: `PaymentEvent{Status}`, the same-component `MailerAgent`
  (`Mailer`, `MailerID`, `Remind`, implemented), `inDays`,
  `createPaymentLink`, `sendSurvey`.
- `agents-stream`: `ctx`, `ai = openai.NewClient()`.
- `artifacts-overview`: `ctx`, `g = genkit.Init(ctx,
  genkit.WithPlugins(&openai.OpenAI{}))`, `const model = "openai/gpt-4.1"`
  (the TS prelude's `model`), the tool input
  ``Quarter{Quarter string `json:"quarter" jsonschema:"pattern=^Q[1-4]-\d{4}$"`}``
  (the TS `Quarter` schema), `chartPng` (stub) and `pageFor`.
- `artifacts-routes`: `Presenter` (tool identity type),
  ``Args{Name golem.Text `golem:"regex=^[a-z0-9-]{1,40}$"`, HTML string}``
  (the TS `Name` schema; the host checks the regex before the command
  runs), `Row{Week, Total}`, `querySales`.
- `artifacts-files`: `SaleRow`, `querySales(team, quarter)`, `drawChart`.
- `artifacts-login`: `Png = golem.UnstructuredBinary[pngType]` with the
  `MimeTypes()` marker (hidden here, shown in artifacts/files, for lack of
  lines), `TeamID{Team}`, `Chart{AskedBy, PNG}`, the state
  `book{Charts map[int32]Chart}` (exported field so a JSON snapshot keeps
  it), `answerWithChart` (stub) and `userKey` (OIDC issuer + subject as a
  JSON array; panics for any other principal, as the TS prelude throws).
- `tools-overview`: `Summarizer` (tool identity type), `Args{URL}`,
  `DigestID{Topic}`, `BuildIn{URLs}` (the TS `Urls` prelude schema), the
  empty agent state `digest`, `summarizeText` (HTTP POST).
- `tools-custom`: `Outliner` (identity type), `Args{File}`, `Headings`
  (= `[]string`), `BuildID{Project}`.
- `tools-requests`, `tools-permissions`, `tools-secrets`, `tools-publish`,
  `agents-readonly`: nothing (no companion.go; tools-secrets and
  tools-publish get their supporting code from bridge stubs).

## How display files are spliced and checked

`check.sh`:

1. Checks the 17-feature list (above). Finds every display file with
   `find website/snippets -path './*/*/go/*'` and reads it in place, writing a
   marker-stripped copy to a temp dir (`sed -E 's,[ ]+(//|#)@$,,'`) that
   every later step uses. It requires exactly one file per
   `<chapter>/<feature>/go/`, a harness package for each, no harness package
   without a display file, and no `agents/sqlite/go/`.
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
   width.
5. Compiles, printing each command and `rc=`:
   ```
   go version
   GOOS=wasip1 GOARCH=wasm go build ./pkg/<all 16>
   GOOS=wasip1 GOARCH=wasm go vet   ./pkg/<all 16>
   # per snippet, a real Golem component:
   (cd components/<p> && PATH=<golem go>/bin:$PATH \
       go tool componentize-go build -o ../../out/<p>.wasm)
   wasm-tools validate --features all out/<p>.wasm
   go test -count=1 ./pkg/<the 8 with definitions_test.go>
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
| `github.com/openai/openai-go/v3` v3.84.0 (real) | agents/stream (Responses API, `OutputText()`) | The official OpenAI Go SDK, the counterpart of the TS `openai` client; compiles for wasip1 and sends through `net/http`, which the Golem SDK routes via the host. |
| `github.com/firebase/genkit/go` v1.14.0 (real): `genkit`, `ai`, `plugins/compat_oai/openai`, `plugins/anthropic` | agents/models, artifacts/overview | The Go counterpart of the Vercel AI SDK call: one API over two providers (`genkit.GenerateText` with `openai/gpt-4.1`, `genkit.GenerateData[Review]` typed structured output with `anthropic/claude-sonnet-4-5`). Chosen over langchaingo (its Anthropic provider ignores JSON mode) and over the two official SDKs directly (over 31 lines). Compiles into a component (~41 MB). **artifacts/overview** uses its tool calling, the counterpart of the TS `tool`/`generateText`/`stepCountIs`: `ai.NewTool(name, description, fn)` (`ai/tools.go:472`; "can be passed directly to Generate", no registry), `fn` of type `ai.ToolFunc[In, Out] = func(*ai.ToolContext, In) (Out, error)` (`ai/tools.go:36`), `genkit.GenerateText` (`genkit/genkit.go:1624`) with `ai.WithModelName`, `ai.WithPrompt`, `ai.WithTools` (`ai/option.go:294`) and `ai.WithMaxTurns(5)` (`ai/option.go:341`; max tool round trips, an error beyond, where `stepCountIs(5)` stops quietly). The tool's input schema is inferred from `Quarter` (its `jsonschema:"pattern=..."` tag included) and every action run validates the input against it before the function runs (`core/action.go:273`); a scratch test on 2026-10-22 showed `Q3-2026` accepted and `../etc/x`, `Q5-2026` rejected ("Does not match pattern"). Paths are relative to the module root in `$(go env GOMODCACHE)/github.com/firebase/genkit/go@v1.14.0`. |
| `golem.local/bridge/research-agent-guest-client` (stub, `bridge/`) | agents/team | Hand-written stand-in for the client `golem build` generates for a ResearchAgent in another component, shaped after `cli/golem-cli/src/bridge_gen/go/mod.rs`: `DefineFullAgentClient`, `ResearchAgentID{Topic}`, `Facts{Summary, Sources}`, `MustGetResearchAgent`, `MustGather` (plus `Gather`, `TriggerRefresh`, `ScheduleRefresh`, no longer shown). Real SDK calls underneath. |
| `golem.local/bridge/slack-agent-guest-client` (stub) | agents/team | Same, for `SlackAgent{Channel}` with `post(text)`: `MustGetSlackAgent`, `TriggerPost`/`MustTriggerPost`. |
| `golem.local/bridge/refund-tool-guest-client` (stub) | tools/secrets | Stand-in for the client generated for a `refund` tool with a root body, named as `bridge_gen/go/tool.rs` names it: identity type `RefundTool`, `Tool = tool.DefineToolClient[RefundTool]("refund")`, command `Root` with `RootArgs{Charge string, Amount int32, APIKey golem.Secret[string]}` (`to_field_ident("apiKey")` = `APIKey`; a secret parameter is `golem.Secret[inner]`, `bridge_gen/go/type_ref.rs:235`), error `ErrHTTP`, result `Refund{ID, Status}`. |
| `golem.local/bridge/bash-tool-guest-client` (stub) | tools/custom | Stand-in for the client generated for the built-in `bash@0.2.1` release, shaped after `bridge_gen/go/tool.rs`: `tool.DefineToolClient[BashTool]("bash")`, `Run` with `RunArgs{Script, Cwd, Timeout}` (defaults `""`, `600`), result `RunResult{ExitCode, Stdout, Stderr}`. The `Timeout` type (`uint32`) and the result type name are guesses. |
| `golem.local/bridge/crm-find-customer-tool-guest-client` (stub, **labelled stand-in for the generated MCP-import client**) | tools/publish | The generated client is not available offline. The upstream MCP tool `crm_find_customer` (input `{email: string}`, required, `additionalProperties: false`; output schema with string `name` and `plan`) is projected as `crm-find-customer` (`golem-mcp-import/src/tool.rs`: one option per input property, optional `*/*` stdout, result record `{structured, content}`, error `mcp-tool-error` with a string payload). The stub follows `bridge_gen/go/tool.rs` for that projection: `CrmFindCustomerTool`, `Tool = tool.DefineToolClient[CrmFindCustomerTool]("crm-find-customer")`, `Root = Tool.OutputBody[RootArgs, Result]` (an output body, so `Root.Call` returns `(*tool.Invocation[Result], error)`), `RootArgs{Email}` (no `AdditionalProperties` map: `additionalProperties` is false), `ErrMcpToolError`, `Result{Structured ResultStructured{Name, Plan}, Content}`. The consumer API the snippet uses (`Root.Call(func(*RootArgs))`, `Invocation.Collect()`, `Collected.Err`, `Result.Structured.<field>`) is the one in `cli/golem-cli/tests/app/go_mcp_import_consumer.go` and in `docs/src/content/v1.6/invoke/mcp.mdx` ("Call the imported tool", Go tab). Guessed: the result type names and the `Content` variant (cut down to `none` and `streamed`; the real one also has `blocks`). |

Everything else is the real in-tree SDK: `golem.DefineAgent`/`Spec`/`Mount`
(`Path`, `Auth`, `ExposeFiles`), `Method` with `golem.HTTP(POST/GET)`,
`golem.ReadOnly()` and `golem.Description(...)`, `Implement(...).Handle(...)`
with `golem.Adapt`/`Adapt0`/`AdaptUnit`, `golem.DefineEnum`,
`golem.UnstructuredBinary[M]` with a `MimeTypes()` marker and `*T` as an
option (nil → 404), `golem.Principal`/`golem.OidcPrincipal`,
`golem.NewWebhook[T]`/`URL`/`MustAwait`, `MustSchedule` +
`ScheduledInvocation.Cancel`, durable `time.Sleep`,
`reflection.DiscoverAgentTypes` (`Name`, `Methods()[i].Name`)/`DiscoverTools`,
`golem.Secret[string].MustGet`,
`golem.DefineConfiguredAgent` + `ctx.Config(def)`, `tool.DefineTool`/
`Spec{RequiresFilesystem}`/`Body`/`Handle`/`Call`, `tool.Invocation.Collect`,
`golem.Text` with a `golem:"regex=..."` tag on a tool argument,
`golem.PatternGrant`/`Bounds`/`MustDeriveFromWallet`/`MustDerive`/
`LifespanOf`/`Install(AgentHolder)`/`Revoke`.

## Negative controls (neg.sh)

neg.sh runs check.sh, copies each package's `companion.go` (if any) and
spliced `snippet.go` to `neg/<p>/` with one planted mistake, requires
`GOOS=wasip1 GOARCH=wasm go build ./neg/<p>/` to fail, checks there is one
control per package, then deletes `neg/` and prints `NEG_OK`.

| Snippet | Mutation | Compiler error |
|---|---|---|
| agents/overview | `golem.Adapt((*TriageID).triage)` → `golem.Adapt0(...)` | cannot infer S and Out |
| agents/models | `Drafter.Method[WriteIn, Draft]("write")` → `Drafter.Method("write")` | cannot infer In |
| agents/team | `….MustGather()` → `….Gather()` | assignment mismatch: Gather returns 2 values |
| agents/waits | `Remind.MustSchedule(mailer, inDays(1), …)` → `Remind.MustTrigger(mailer, …)` | `reminder.Cancel` undefined (golem.InvocationID) |
| agents/stream | `AgentStream[string]` → `AgentStream[int]` in `Method` | handler type does not match |
| agents/readonly | `golem.ReadOnly())` → `golem.ReadOnly(true))` | cannot use true as golem.CachePolicy |
| tools/overview | `Summarize.Call(func(a *Args) {` → `func(a Args)` | cannot use func(a Args) as func(*Args) |
| tools/custom | `golem.Must(Outline.Call(` → `(Outline.Call(` | too many return values |
| tools/requests | `golem.Must(http.NewRequest(` → `golem.Must0(` | too many arguments in call to golem.Must0 |
| tools/permissions | `golem.Some(golem.LifespanOf(time.Hour))` → `golem.Some(time.Hour)` | Option[time.Duration] is not Option[golem.Lifespan] |
| tools/secrets | `a.APIKey = ctx.Config(Support).StripeKey` → `….StripeKey.MustGet()` | cannot use string as golem.Secret[string] |
| tools/publish | `c := out.Result.Structured` → `c := out.Structured` | out.Structured undefined (tool.Collected[Result]) |
| artifacts/overview | `func(_ *ai.ToolContext, args Quarter) (string, error) {` → `func(args Quarter) ...` | does not match ai.ToolFunc[In, Out] (cannot infer In and Out) |
| artifacts/routes | `}).Body[Args, string](` → `Body[Args, int]` | cannot use func(...) (string, error) as func(*tool.Context, Args) (int, error) |
| artifacts/files | `Charts.Method[ChartIn, Png]("chart",` → `Method[ChartIn, []byte]` | Adapt((*ChartID).chart) returns Png, not the declared []byte |
| artifacts/login | `Auth: true}})` → `Auth: "oidc"}})` | cannot use "oidc" as bool |

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
(`sdks/go/golem/codec_sdk.go:68-99`) encodes a secret by `secretTake()`. For a
config secret, `take` mints a fresh host handle with `get-config-value`
(`secret_wasm.go:55-65`), never the plaintext. Not verified at runtime.

## Departures from the TypeScript (by design)

- **Omitted declarations.** As in TS, schemas and helpers live in the
  harness. Go also hides the tool identity types (`Summarizer`, `Outliner`)
  and the tool argument structs (`Args{URL}`, `Args{File}`); tools/overview
  hides `DigestID{Topic}` and `BuildIn{URLs}` and tools/custom hides
  `BuildID{Project}` (TS writes those ids inline; needed to stay within 31
  lines).
- **Handler shape.** Agents chain `Implement(init).Handle(Method,
  golem.Adapt…(…))`; the state is the agent's ID struct where TS keeps only
  the id's fields, and an empty struct where TS returns `{}`.
- **Exported names.** TS `export`ed values are exported in Go. Agent and
  method descriptors cannot share a name, so TS `Triage`/`triage` is
  `Triager`/`Triage` and `Plan`'s result type is `PlanResult`.
- **agents/overview** marks the ID struct (`type TriageID struct{ Inbox
  string }`) for TS `id: { inbox }` and the input struct for TS `input: {
  emails }`, with the id comment above the ID type. TS `Label.parse(await
  classify(...))` is `parseLabel(classify(...))`; `Label` is a `uint8`
  `golem.DefineEnum` (a plain `int` is rejected: platform-dependent width).
- **agents/team** keeps "a Rust ResearchAgent" (the call is cross-language
  from Go). The awaited call spans two lines (both marked); the discovery is
  a loop with an inner loop over `a.Methods()`, marked on the
  `DiscoverAgentTypes` line, the `name: methods` line and the
  `DiscoverTools` line.
- **agents/waits** marks both lines of the schedule (`mailer := Mailer.Get`
  and `Remind.MustSchedule`), as TS marks both lines of its statement.
- **agents/stream** puts its comment inside the `Method(...)` call, above
  the route argument, because a decl-level comment would force a 32nd line.
- **agents/readonly** puts its three comments inside the `level`
  `Method(...)` call, above `golem.ReadOnly()`, as TS does above
  `readOnly`. `z.int()` is `int32`.
- **tools/overview** declares the handler as a top-level `func summarize`
  (registered with `Summarize.Handle(summarize)`), with "each call gets a
  fresh VM that can't read the agent's memory:" above it, and marks the
  `func` line (TS marks `summarize: async ({ url }) => {`). The fetch is
  `http.Get` + `io.ReadAll`, marked on the `http.Get` line. The tool is
  defined as `Tool` then `Summarize = Tool.Body[...]` to fit 68 columns.
- **tools/requests** marks the URL line and the `Do` line (TS marks the one
  `fetch` line).
- **tools/publish** uses `golem.Description(...)` (the docs' prose calls it
  `golem.Desc`, but the code and SDK use `Description`). The generated
  client is called `crm.Root.Call(func(a *crm.RootArgs) { a.Email = … })`
  (Go tool clients name the root command `Root`, where TS writes
  `CrmFindCustomerClient.newClient().crm_find_customer(email)`); both lines
  are marked. `result.status === 'rejected'` → `golem.Must0(out.Err)`. No
  import line is shown (TS shows the generated-client import; Go's rules keep
  imports in the harness).
- **Artifacts chapter (all four).** Every artifact is made in the call
  that answers a user's request (owner ruling, 2026-10-22); nothing is
  deployed, built or static. File writes use `os.MkdirAll`/`os.WriteFile`
  wrapped in `golem.Must0` (a failure traps the invocation, like an uncaught
  TS throw); TS `fs.mkdirSync`/`fs.writeFileSync`.
- **artifacts/overview** marks the `ExposeFiles` entry (`{Route: "/*",
  Path: "/out/$1"}`, a subtree mapping, `sdks/go/golem/http_files.go:33-36`),
  the `chart := ai.NewTool("chart",` line (TS `const chart = tool({`) and the
  first line of each `os.WriteFile` (two lines each; the marker is on the
  call). The tool is a Genkit `ai.NewTool` closure over the agent (TS
  `tool({...})` closing over `this.chat`); its input is the hidden `Quarter`
  struct (`args.Quarter`, TS `{ quarter: q }`), validated by Genkit against
  the struct's pattern before the function runs (TS: the AI SDK validates
  `inputSchema`). The loop is `genkit.GenerateText(ctx, g,
  ai.WithModelName(model), ai.WithPrompt(...), ai.WithTools(chart),
  ai.WithMaxTurns(5))`; `WithMaxTurns(5)` errors after 5 tool round trips
  where TS `stopWhen: stepCountIs(5)` stops after 5 steps. The description
  is the second argument of `ai.NewTool`.
- **artifacts/routes** (`app_tool.go`) puts `AppID` and `SalesIn` first:
  gofmt forces a blank line between a `type` group and a `var` group, and
  this order needs only one, which keeps the file at 31 lines. The tool is
  `tool.DefineTool[Presenter]("present", tool.Spec{RequiresFilesystem:
  true}).Body[Args, string]` with two `s.Positional` lines (TS
  `toolDefinition(...).body(b => b.positional(...)...returns(...))`); the
  handler returns `(string, error)` (TS `ok(...)`), and converts the
  `golem.Text` name once (`name := string(a.Name)`). Marked: the
  `os.WriteFile` call and the `golem.HTTP(golem.GET("/sales?region=
  {region}"))` line; `RequiresFilesystem` is not marked. The constructor is
  a one-line closure so the `Implement(...).Handle(` line fits 68 columns.
- **artifacts/files** declares the PNG type as `pngType` + `MimeTypes()` +
  `type Png = golem.UnstructuredBinary[pngType]` (TS
  `s.unstructuredBinary({ mimeTypes: ['image/png'] })`), and the handler as
  a method `func (c *ChartID) chart(in ChartIn) Png` registered with
  `golem.Adapt((*ChartID).chart)`. TS `returns: Png //@` sits inside the
  method options; in Go the return type is in `Method[ChartIn, Png]` and in
  the handler signature, so the comment "the reply is the PNG itself, over
  HTTP or MCP:" and the marker are on the handler's `func` line (keeping
  the TS comment order: the MCP-description comment comes first, inside
  `Method(...)` above `golem.Description(...)`). The other markers: the
  `drawChart` line and the inline `return Png{Data: val, MimeType:
  "image/png"}` (TS `{ tag: 'inline', val, mimeType }`; Go inline = `Data`
  set, `URL` empty).
- **artifacts/login** hides `Png` (with its marker type) and `TeamID` in
  the companion for lack of lines (both are shown in artifacts/files). The
  inputs are `AskIn{Question, Caller golem.Principal}` and `OpenIn{N int32,
  Caller golem.Principal}` (TS `s.principal()`; the host fills the field,
  `docs/.../golem-add-http-auth-go.mdx:66`); TS `z.number()` is `int32`.
  The state is `book{Charts map[int32]Chart}` keyed by chart number (TS an
  array indexed `n - 1`), so a missing number reads the zero `Chart`, whose
  empty `AskedBy` never equals a user key: the Go counterpart of TS
  `made?.askedBy !== userKey(caller)`. Two methods need the
  implementation handle (`var impl = PrivateCharts.Implement(...)`, then
  one `var _ = impl.Handle(...)` per method; `Handle` returns
  `golem.Registered`, not the handle). The owner check is written
  positively as an `if` with an initializer that returns the PNG (marked),
  then `return nil` (404), where TS returns `undefined` on mismatch first;
  this saves the line needed to stay within 31.
- **Errors.** TS `throw` becomes `panic` (traps the invocation, like an
  uncaught throw); TS `.unwrap()`/rejections become `golem.Must`/`Must0`.

## Known limits

- Components are built and validated, but nothing was deployed or run on a
  Golem server.
- `golem.DefinitionErrors()` runs natively on every check.sh run for the 8
  packages that link without host imports (agents-readonly, artifacts-files,
  artifacts-login, artifacts-routes, tools-custom, -permissions, -requests,
  -publish); this checks the HTTP bindings, file mappings, the
  unstructured PNG returns and the `present` tool's `regex` restriction. The
  others reach `net/http` (the SDK's host-backed transport) or RPC and fail
  to link natively. On 2026-10-22 agents-overview (HTTP helpers stubbed),
  tools-overview (fetch and summarizeText stubbed) and the old
  artifacts-overview (model call stubbed) passed a one-off native
  definitions test; it caught
  the enum width problem (`type Label int` → `uint8`).
- genkit's runtime behaviour inside a Golem component was not exercised;
  only compilation. The tool-input pattern check was run natively in a
  scratch module (not inside Golem). The host-side enforcement of the
  `present` tool's `regex` restriction is documented
  (`docs/src/content/v1.6/how-to-guides/go/golem-define-tool-go.mdx:122`,
  "The host checks the restrictions before the command runs") but was not
  run.
- check.sh and neg.sh share the generated `pkg/*/snippet.go`, `components/`
  and `out/`: do not run two of them at once.
- The bridge stubs approximate generated code; regenerate against real
  callee schemas before relying on exact names or types.
