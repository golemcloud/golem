# Scala home-page snippets: how they are compiled

This directory proves that every Scala per-feature display snippet on the home
page compiles and links against the in-tree Golem Scala.js SDK (`sdks/scala`),
including the Golem macros (agent/tool/HTTP metadata validation) and the Golem
sbt codegen (RPC and tool clients, auto-registration). Verified at repository
commit `d35a841e6` (with uncommitted snippet edits; re-verified Oct 22 2026
after the chapter reorganization: durable streaming and read-only methods
moved to Agents, "Your language" dropped, new TypeScript designs ported,
and `tools/publish` became a code snippet; re-verified again Oct 22 2026
after the Artifacts chapter redesign: artifacts are made on request,
`artifacts/routers` was replaced by `artifacts/files`; re-verified again
Oct 22 2026 after the 7 + 7 + 7 reorganization: new `tools/middleware`,
`artifacts/peruser`, `artifacts/routers` (a raw router again) and
`artifacts/signin`, small edits to agents/overview, tools/publish,
artifacts/overview, artifacts/routes and artifacts/login) on macOS arm64.

## Display files (the only inputs; read in place, never copied by hand)

`check.sh` discovers every file matching `website/snippets/*/*/scala/*`:

| Chapter | Feature | File |
|---|---|---|
| agents | overview | `website/snippets/agents/overview/scala/TriageAgent.scala` |
| agents | models | `website/snippets/agents/models/scala/DraftAgent.scala` |
| agents | team | `website/snippets/agents/team/scala/PlannerAgent.scala` |
| agents | waits | `website/snippets/agents/waits/scala/CheckoutAgent.scala` |
| agents | stream | `website/snippets/agents/stream/scala/WriterAgent.scala` |
| agents | readonly | `website/snippets/agents/readonly/scala/StockAgent.scala` |
| tools | overview | `website/snippets/tools/overview/scala/DigestAgent.scala` |
| tools | custom | `website/snippets/tools/custom/scala/BuildAgent.scala` |
| tools | requests | `website/snippets/tools/requests/scala/StripeRefund.scala` |
| tools | permissions | `website/snippets/tools/permissions/scala/OrdersAccess.scala` |
| tools | secrets | `website/snippets/tools/secrets/scala/SupportAgent.scala` |
| tools | middleware | `website/snippets/tools/middleware/scala/RefundPolicy.scala` |
| tools | publish | `website/snippets/tools/publish/scala/AccountAgent.scala` |
| artifacts | overview | `website/snippets/artifacts/overview/scala/AnalystAgent.scala` |
| artifacts | routes | `website/snippets/artifacts/routes/scala/AppAgent.scala` |
| artifacts | peruser | `website/snippets/artifacts/peruser/scala/DashboardAgent.scala` |
| artifacts | files | `website/snippets/artifacts/files/scala/ChartAgent.scala` |
| artifacts | routers | `website/snippets/artifacts/routers/scala/ExportsRouter.scala` |
| artifacts | signin | `website/snippets/artifacts/signin/scala/BillingAgent.scala` |
| artifacts | login | `website/snippets/artifacts/login/scala/PrivateChartsAgent.scala` |

There is deliberately no `agents/sqlite/scala` (Scala agents have no embedded
SQLite; the page shows the chapter's `missing.sqlite` note), so the Scala set
is the 21 TypeScript features minus sqlite: 20 files. The Scala names match
the TypeScript agent/tool names (`TriageAgent`, `StockAgent`, tool
`summarize` + `DigestAgent`, `AccountAgent`, `AnalystAgent`, tool
`present` + `AppAgent`, `ChartAgent`, `PrivateChartsAgent`, middleware
`refund-policy`, `DashboardAgent`, `ExportsRouter`, `BillingAgent`, ...).

### Highlight markers

A display line ending in ` //@` is highlighted on the page while its
snippet shows; the page strips the marker and it never counts toward the
68-character limit. Every marked TypeScript line has a marked Scala
counterpart (the same Golem operation or declaration). Comment lines can be
marked too (tools/secrets marks its two comment lines, as TypeScript does).

## Prerequisites

| Tool | Version used | Notes |
|---|---|---|
| JDK | OpenJDK 26.0.1 (Homebrew) | SDK requires JDK 21+ |
| sbt launcher | 1.12.11 (Homebrew, `/opt/homebrew/bin/sbt`) | the build runs sbt 1.12.8 from `project/build.properties` |
| Scala | 3.8.2 | `build.sbt`; downloaded by sbt |
| Scala.js sbt plugin | 1.22.0 | `project/plugins.sbt` |
| Golem sbt plugin | `cloud.golem % golem-scala-sbt % 0.0.0-SNAPSHOT` | local publication, see below |
| SDK libraries | `cloud.golem %%% golem-scala-core / golem-scala-model % 0.0.0-SNAPSHOT`, `cloud.golem %% golem-scala-macros % 0.0.0-SNAPSHOT` | local publication |
| scalajs-dom | 2.8.1 | `fetch`, `RequestInit`, `Response`, `URLSearchParams` |
| zio-blocks | 0.0.54 (transitive from the SDK) | `Schema`, `Stream` |

No npm install, wasi-sdk, wasm-rquickjs or environment variables are needed:
Scala.js `@JSImport` facades only emit ES-module `import` statements, so
`node:fs`, `node:timers/promises` and `openai` need not be resolvable at
compile/link time.

The organization is `cloud.golem` (`sdks/scala/build.sbt`), not `dev.zio`.

### Publishing the in-tree SDK locally (once, or after SDK changes)

From the repository root (canonical; also regenerates the guest runtime WASM
and syncs WIT):

```shell
rm -f sdks/scala/target/.local-publish-marker   # only when SDK inputs changed
cargo make build-sdk-scala
```

That runs, in `sdks/scala`:

```shell
./scripts/generate-agent-guest-wasm.sh
sbt 'set every version := "0.0.0-SNAPSHOT"' \
  '+modelJVM/publishLocal' '+modelJS/publishLocal' '+macros/publishLocal' \
  'core/publishLocal' '+codegen/publishLocal' '++2.12.21' 'sbtPlugin/publishLocal'
```

Lighter alternative when the guest WASM already exists:
`cd sdks/scala && sbt golemPublishLocal`. Artifacts land in
`~/.ivy2/local/cloud.golem/`. The run recorded here used the publication of
Oct 19 2026 16:37 local time, newer than the last commit touching
`sdks/scala` (`610ae1912`, Oct 19 2026 09:28 UTC); `git status sdks/scala`
was clean.

## Harness layout

```
.amp/in/snippets/scala/
  build.sbt                 Scala.js + GolemPlugin project; -experimental -deprecation -Werror;
                            ES modules; golemBasePackage = "check"
  project/build.properties  sbt.version=1.12.8
  project/plugins.sbt       golem-scala-sbt 0.0.0-SNAPSHOT, sbt-scalajs 1.22.0
  harness/
    pre/<chapter>-<feature>.scala    prelude per snippet: package check.<chapter>.<feature>,
                                     imports, and the helpers the snippet uses without
                                     defining (like the TS preludes)
    extra/<chapter>-<feature>.scala  other agents a snippet calls (own file, same package;
                                     no top-level definitions: the hyphenated file name
                                     makes Scala warn, which -Werror turns into an error)
    lib/openai.scala        facade over the npm `openai` client, including its runTools
                            tool loop (see below)
    lib/ai.scala            invented `ai` library (Vercel AI SDK port), fails at runtime
    lib/aisdk.scala         invented `aisdk.openai` / `aisdk.anthropic` providers
    lib/node.scala          `node.fs` (node:fs, text and Uint8Array reads and writes) and `node.sleep`
                            (node:timers/promises) facades
    lib/bash.scala          stub of the generated bash tool client (golem.bridge.client.bash)
    lib/crm.scala           stub of the generated client for the MCP-imported tool
                            crm_find_customer (golem.bridge.client.crm_find_customer)
    lib/support.scala       `homepage.orThrow` (Either unwrap, mirrors TS `.unwrap()`)
  check.sh                  verification; ends with ALL_OK
  neg.sh                    negative controls; ends with NEG_OK
  out/                      sbt.log, neg-*.log, stripped/ (marker-free display copies; regenerated)
  src/                      generated sources (regenerated by check.sh; never edit)
```

## How display files are spliced

`check.sh` (env `SNIPPETS_ROOT` overrides `website/snippets`; neg.sh uses it):

1. Finds every display file `*/*/scala/*`, strips each trailing highlight
   marker (`sed -E 's,[ ]+(//|#)@$,,'`) into `out/stripped/<same path>`,
   counts the markers, and on the stripped text checks:
   width ≤ 68, no tabs, every `//` comment on its own line and ending with
   `:` (string literals are blanked first), no `/*` comments, ≤ 31 lines. It
   prints each file's line count, longest line and marker count. A marker
   left anywhere but at the end of a line fails the own-line comment check.
2. Checks the feature set equals the TypeScript originals'
   (`*/*/ts/*.ts`, 21 features) minus `agents/sqlite`, with exactly one
   Scala file per feature and no `agents/sqlite/scala`. There is no YAML
   display file any more (`tools/publish` is code).
3. Writes `src/main/scala/check/<chapter>/<feature>/<DisplayName>.scala` =
   `harness/pre/<chapter>-<feature>.scala` followed by the stripped display
   file (`cat pre out/stripped/<display> > out`).
   It prints the prelude's line count so compiler line numbers map back to
   display lines (display line = reported line − offset). It copies
   `harness/extra/*.scala` and `harness/lib/*.scala` alongside.
   Each snippet gets its own package, so the snippets cannot see each other.
4. Runs, in this directory:

   ```shell
   sbt -batch -no-colors -Dsbt.supershell=false clean fastLinkJS
   ```

   i.e. Golem codegen (RPC clients such as `ResearchAgentClient`,
   `MailerAgentClient`, tool clients `OutlineClient`, `RefundToolClient`,
   auto-registration of every agent/router/tool implementation), compilation
   with the Golem macros under `-Werror`, and Scala.js linking of the ES
   module. It prints the exit code and, per display file, PASS or the number
   of compiler errors/warnings located in its spliced file. `ALL_OK` only if
   every step passed. Takes about 20–30 s warm.

The `error: .../.scalafmt.conf: Missing config` and
`[warn] [golem] scalafmt failed ...` lines come from the Golem codegen
formatting its generated files; they are harmless sbt log lines, not compiler
diagnostics, so `-Werror` is unaffected.

## Invented libraries, facades and stubs

| Import path | Source | Kind | Why |
|---|---|---|---|
| `ai.{generateText, Output}` (`LanguageModel`, `GenerateTextResult`) | `harness/lib/ai.scala` | invented; type-correct, throws `NotImplementedError` | agents/models uses the Vercel AI SDK (`ai`). No Scala/Scala.js port exists, so the API mirrors the TS one: `generateText(model, prompt)` and `generateText(model, output = Output.`object`[A], prompt)` return `Future[GenerateTextResult[A]]` with `text` and typed `output` (schema from zio-blocks `Schema[A]`). The TS snippet shows its imports, so the Scala one shows `import ai.{generateText, Output}`. |
| `aisdk.openai.openai`, `aisdk.anthropic.anthropic` | `harness/lib/aisdk.scala` | invented | the `@ai-sdk/openai` and `@ai-sdk/anthropic` providers; they only name a model. |
| `openai.{OpenAI, ChatParams, ChatMessage, responseFormat, ResponseParams, FunctionTool, ToolRunParams, RunnerOptions}` | `harness/lib/openai.scala` | hand-written Scala.js facade over the real npm `openai` client (reused from the previous harness, extended) | agents/overview uses `openai.chat.completions.parse(...)` and `message.parsed`; agents/stream uses `openai.responses.create(...)` and `output_text`, as the TS snippets do. `responseFormat[A](name)` is the analogue of `zodResponseFormat`, really implemented from `Schema[A].toJsonSchema`. artifacts/overview uses the client's real automated tool loop `openai.chat.completions.runTools(params, { maxChatCompletions })` and `runner.finalContent()` (openai-node `docs/helpers.md`, "Automated function calls"; `src/resources/chat/completions/completions.ts`, `runTools`); `FunctionTool(...)` only builds the client's `{ type: 'function', function: { name, description, parameters, parse, function, strict } }` object, `ToolRunParams`/`RunnerOptions` the two argument objects. Imported as `_root_.openai.{...}` because the snippets' client value is also called `openai`. |
| `node.fs` | `harness/lib/node.scala` | facade over `node:fs` (provided by the Golem QuickJS runtime; the approach documented in `golem-skills/skills/scala/golem-file-io-scala`) | `readFileSync` (text and `Uint8Array`), `writeFileSync` (text, and the `Uint8Array` overload artifacts/overview uses for the PNG), `mkdirSync`. Snippets pass `literal(recursive = true)` (`scala.scalajs.js.Dynamic.literal`, imported by the prelude). |
| `node.sleep` | `harness/lib/node.scala` | facade over `setTimeout` from `node:timers/promises`, returned as a `Future` | the same durable sleep the TS waits snippet imports; the Scala SDK has no sleep API of its own. |
| `golem.bridge.client.bash.{BashClient, BashResult, BashError}` | `harness/lib/bash.scala` | stub of a generated client; throws at runtime | stands in for the client `golem build` generates for the built-in bash tool (`golem-temp/bridge-sdk/scala/internal/bash-tool-guest-client`). Shape from `cli/golem-cli/src/bridge_gen/scala/tool.rs` (`BashClient()` / `newClient()`, `Future[Either[ToolError[E], A]]`, u8 → `golem.UByte`, u32 → `golem.UInt`), the call in `golem-call-tool-scala` (`run(script, cwd, UInt(600))`) and the result/error types in `builtin-tools/bash/component/src/lib.rs`. |
| `golem.bridge.client.crm_find_customer.CrmFindCustomerClient` | `harness/lib/crm.scala` | **stub, clearly labelled stand-in for the generated client**; throws at runtime | tools/publish calls the tool `crm_find_customer` imported from an MCP server (`mcp.imports`); `golem build` would generate this client, which is not available offline. Shape from `cli/golem-cli/src/bridge_gen/scala/tool.rs` (`object …Client` with `apply()`/`newClient()`; package `golem.bridge.client.<snake name>`; leaf method = lower-camel tool name with one parameter per input property and no additional-properties map, since the schema has `additionalProperties: false`; the projected command has stdout, so the method returns `Either[ToolError[E], golem.tool.ToolInvocation[E, R]]` synchronously, consumed with the real SDK `ToolInvocation.collect()` → `CollectedToolInvocation.result: Either[ToolError[E], R]`), from `golem-mcp-import/src/tool.rs` (`R` = record `{ structured, content }`, `structured` typed from the outputSchema; error case `mcp-tool-error` with a string payload), and from the docs' Scala tab (`docs/src/content/v1.6/invoke/mcp.mdx`, "Call the imported tool"). The Scala *names* of the anonymous result/structured/content records and the content type are placeholders (the snippet never names them). The display shows the client's import, as the TypeScript does. |
| `homepage.orThrow` | `harness/lib/support.scala` | helper | unwraps `Either` results (`PermissionError`, `GolemReflectError`, `ToolError`) or throws, like TS `.unwrap()`; shown undefined in the snippets. |

Other agents a snippet calls are real Scala agents in `harness/extra/`, so
the Golem codegen generates their clients exactly as for any agent:
`ResearchAgent` and `SlackAgent` (agents/team; in production ResearchAgent is
a Rust agent whose guest bridge client, generated by
`cli/golem-cli/src/bridge_gen/scala/mod.rs`, has the same `get`, awaited
`apply` and `trigger` surface),
and `MailerAgent` (agents/waits), and the refund tool `RefundTool` that
tools/secrets calls (`harness/extra/tools-secrets.scala`; `Refund` and
`stripeRefund` are in that snippet's prelude, since the refund tool is no
longer a display snippet). Both tools/overview and tools/secrets are
self-contained packages. tools/middleware wraps an identical copy of that
refund tool (`harness/extra/tools-middleware.scala`, its own package):
`refund(charge, amount, apiKey: GuestSecretHandle, caller: Principal)`
returning `Refund(id, status, customerEmail)` or `RefundError.Http`. The
`caller: Principal` parameter is injected by Golem and is not part of the
tool's input or of the generated client (`RefundToolClient().refund(charge,
amount, handle)` is unchanged); it is how a Scala typed middleware sees the
caller principal (see "Departures"). artifacts/routers calls the user's
`AnalystAgent` (`harness/extra/artifacts-routers.scala`, `class Id(val chat:
String)`, `def exportCsv(id: String): AgentStream[Array[Byte]]`), from which
the codegen generates `AnalystAgentClient`.

Prelude helpers shown without definition, as in the TS preludes: `classify`
(returns the label name, parsed with `Label.valueOf` as TS uses
`Label.parse`) and `applyLabel` (agents/overview); `Facts`, `Plan` (team);
`PaymentEvent`, `inDays`, `createPaymentLink`, `sendSurvey`, `sleep`
(waits); `openai` (agents stream, artifacts overview); `Quarter`
(the chart tool's JSON schema with the `^Q[1-4]-\d{4}$` pattern, and the
`parse` function runTools applies to the model's arguments, which rejects
any other quarter before it reaches a path), `chartPng`, `pageFor`
(artifacts overview); `Name` (the slug regex `^[a-z0-9-]{1,40}$`),
`BadName` (the tool's usage error enum), `Row`, `querySales` (artifacts
routes); `SaleRow`, `querySales`, `drawChart` (artifacts files); `Chart`,
`answerWithChart`, `userKey` (OIDC issuer + subject as a JSON array, as in
TS; throws for non-OIDC callers) (artifacts login); `Refused` (an
exception carrying a `ToolInvokeError[Nothing]`), `askFinance`,
`approved` (decodes the promise payload's JSON `approved` flag),
`checkRateLimit` (fails with `Refused(ConstraintViolation(...))` over the
caller's limit), `audit` (tools middleware); `dashboardFor` (artifacts
peruser); `empty()` (an empty `AgentStream[Array[Byte]]`) and the
package-level `AgentStream` alias (artifacts routers); `Invoice`,
`invoicesOf`, `pageFor` (artifacts signin); `Headings` (custom);
`summarizeText` and `Urls` (tools overview); `AgentId` =
`HostApi.AgentIdLiteral` (permissions); `orThrow` (secrets, custom, team,
permissions, tools overview, publish).

## Real Golem Scala SDK APIs used

`@agentDefinition(mount, auth, exposeFiles)` (artifacts overview and
routes: the live-file mapping `("/*", "/out/$1")`, as in
`sdks/scala/macros/src/test/scala-3/golem/runtime/macros/HttpRouterMacroSpec.scala:36`
and `docs/src/content/v1.6/invoke/making-custom-apis/http-routers.mdx:391`;
login: `auth = true`), `@agentImplementation`,
`@endpoint(method, path)` including a query variable
(`/chart.png?quarter={quarter}`, `/sales?region={region}`), `@readOnly`
(on a method without an HTTP endpoint), `@description` on an agent method
(tools/publish, artifacts/files; the MCP tool description per
`docs/src/content/v1.6/invoke/mcp.mdx:125,197`),
`golem.schema.UnstructuredBinary["image/png"]` returned as `Future[...]`
(files) or `Option[...]` (login) from a GET endpoint (raw body, `None` →
404; see `sdks/scala/docs/unstructured.md`; MCP binary content per
`docs/src/content/v1.6/invoke/mcp.mdx:254`),
`golem.tool.ToolInvocation.collect()`,
Scala 3 `enum ... derives Schema` and `Map[String, Label]` results,
`golem.Principal` / `Principal.Oidc` (`sub`, `issuer`) method injection,
`golem.schema.AgentStream.fromPull` (synchronous `AgentStream[String]`
returns), `HostApi.createWebhook()` with `.url` and
`.await().map(_.json[A]())`, generated clients `XClient.get(...)`, awaited
calls, `.trigger(...)`, `.scheduleCancelableAt(when)`
(→ `CancellationToken.cancel()`), `golem.Datetime.afterSeconds`,
`golem.reflection.Reflection.getAllAgentTypes()` (`name`, `methods.map(_.name)`)
and `getAllToolTypes()` (`name`), `@toolDefinition(name, version,
requiresFilesystem)` (`sdks/scala/model/src/main/scala/golem/runtime/annotations/ToolAnnotations.scala:25-29`;
artifacts/routes declares tool `present` with it and returns
`Either[BadName, String]`), `@toolImplementation`, `@error(kind, exitCode)`
on a tool error enum, generated tool clients `OutlineClient()` /
`SummarizeClient()` / `RefundToolClient()` (typed `apiKey: GuestSecretHandle`),
`golem.schema.GuestSecretHandle` +
`golem.host.SecretApi.reveal[String]`, `golem.config.{AgentConfig, Config,
Secret}` (`AgentConfig[T]` on the trait, `Config[T]` constructor
parameter, `Secret[String].handle()` for an owned handle without revealing),
and `golem.host.PermissionsApi.{PatternGrant, Bounds,
deriveFromWallet, PersistentCard.derive/install/revoke, Lifespan.iso8601,
Holder.Agent}`.

New in the 7 + 7 + 7 round:

- **Typed tool middleware** (tools/middleware):
  `@toolMiddleware(name = "refund-policy")`
  (`sdks/scala/model/src/main/scala/golem/runtime/annotations/ToolAnnotations.scala:39`)
  on a no-argument class extending the codegen-generated
  `RefundToolMiddleware` (`sdks/scala/codegen/src/main/scala/golem/codegen/rpc/ToolMiddlewareCodegen.scala:164-170`;
  docs `docs/src/content/v1.6/how-to-guides/scala/golem-tools-middleware-scala.mdx:12-18`).
  The handler is `refund(underlying: RefundToolUnderlying, charge, amount,
  apiKey, caller: Principal): Future[Either[ToolInvokeError[RefundError],
  Refund]]`; the principal is decoded from the invocation context
  (`sdks/scala/model/src/main/scala/golem/tool/ToolMiddleware.scala:305`)
  and dropped from the underlying's parameters
  (`sdks/scala/codegen/src/main/scala/golem/codegen/rpc/ToolProjectionIR.scala:547`).
  Forwarding is `underlying.refund(charge, amount, apiKey).toMiddlewareResult`
  (`ToolMiddleware.scala:148`); rejection is
  `ToolInvokeError.ConstraintViolation(message)`
  (`sdks/scala/model/src/main/scala/golem/tool/ToolInvoke.scala:53`).
  Auto-registration emits
  `ToolMiddlewareImplementation.registerTransparent[RefundTool,
  RefundToolUnderlying, RefundToolMiddleware, RefundPolicy]`
  (`sdks/scala/core/js/src/main/scala-3/golem/runtime/autowire/ToolMiddlewareImplementation.scala:36`).
- **Promises** (tools/middleware): `HostApi.createPromise(): PromiseId` and
  `HostApi.awaitPromise(id): Future[Array[Byte]]`
  (`sdks/scala/core/js/src/main/scala/golem/HostApi.scala:299,315`; docs
  `docs/src/content/v1.6/develop/promises.mdx:54-60`).
- **Raw HTTP router** (artifacts/routers): `@httpRouter(typeName, mount)`
  and `@httpHandler`
  (`sdks/scala/model/src/main/scala/golem/runtime/annotations/HttpRouter.scala:11,21`)
  on a trait extending `BaseAgent`, implemented by an
  `@agentImplementation` class;
  `golem.runtime.http.{HttpRequest, HttpResponse, HttpHeader.ascii}`
  (`sdks/scala/model/src/main/scala/golem/runtime/http/HttpExchange.scala:14,40,52`;
  body `AgentStream[Array[Byte]]`); docs
  `docs/src/content/v1.6/invoke/making-custom-apis/http-routers.mdx:118-133`
  (router), `:297-304` (`HttpHeader.ascii`), `:736-741` (calling an
  agent's generated client from a router and mapping its result).
  The router-to-agent call is the codegen-generated
  `AnalystAgentClient.get(chat).exportCsv(id)` returning
  `Future[AgentStream[Array[Byte]]]`, passed through as the response body.
- **Per-user agent from the URL** (artifacts/peruser): `mount =
  "/dashboards/{user}"` bound to `class Id(val user: String)` with
  `exposeFiles = Array(("/*", "/out/$1"))`, as in artifacts/overview.
- **Sign-in** (artifacts/signin): `auth = true` and `Principal` injection,
  matched on `Principal.Oidc` (`issuer`, `sub`;
  `sdks/scala/model/src/main/scala/golem/Principal.scala:24-35`).

## Scala-specific workarounds

- Generated RPC clients copy agent-method signatures but not the source
  file's imports, so `AgentStream` is aliased at package level in the
  agents/stream prelude (`type AgentStream[+A] =
  golem.schema.AgentStream[A]` plus the companion `val`). `Headings` (tools
  custom) is a package-level alias for the same reason.
- Agents without constructor parameters declare `class Id()` (`Refunder`).
- `Datetime.afterSeconds` takes a `Double`; `inDays` passes
  `days * 86400.0` (an `Int * Long` argument is a deprecation error under
  `-deprecation -Werror`).

## Departures from the TypeScript (all forced by Scala or the Scala SDK)

- No early `return` in `Future` code; sequential `for await` loops become
  `foldLeft` over `Future`s (agents/overview, tools/overview), so each
  iteration still waits for the previous one, as in TypeScript.
- agents/overview: `Label` is a Scala 3 enum (`Urgent, Billing, Other`)
  rather than the lowercase string enum; the TS `id: { inbox }` marker sits
  on `class Id(val inbox: String)` and the `input: { emails }` marker on the
  `triage` signature.
- agents/waits: the wait chain is a `for` over `Future`s; the
  "not succeeded" early return becomes `if ... then Future.unit else`.
  `sleep` is the `node:timers/promises` facade (prelude), not an import.
  Both lines of the reminder statement are marked, as in TS.
- agents/team: `Reflection.getAll*Types()` return `Either`, unwrapped with
  `orThrow`; the awaited call is `.map: facts =>` on the client future
  (marked), and `trigger` returns a discarded `Future`, as TS discards its
  result. The comment keeps "a Rust ResearchAgent".
- agents/readonly: the three comments precede `@readOnly` on `level()`
  (TS: inside `method({...})` before `readOnly: true`). Scala has no `init`;
  `units` is a `private var`.
- tools/overview: the tool returns `Future[String]` (no error enum; TS
  `ok(...)`); the client call unwraps the `Either` with `.map(_.orThrow)`.
- tools/custom: the generated tool clients return `Either`s, unwrapped
  with `orThrow`; bash's `exitCode` is a `golem.UByte` (`.value`).
- tools/requests: the `Authorization` header is split over two lines to
  fit 68 columns.
- tools/publish: TS destructures `{ name, plan }`; Scala binds
  `val found = ….structured` and reads `found.name` / `found.plan`. TS
  `if (result.status === 'rejected') throw result.reason` is
  `collected.result.orThrow`; starting the call returns an `Either`
  (`.orThrow`) in Scala, while TS starts it infallibly. The display shows
  the client import, like the TS snippet (and agents/models).
- agents/stream: TS async generators become
  `AgentStream.fromPull(() => Future[Option[A]])`.
- artifacts/overview: Scala has no Vercel AI SDK, so the model loop is the
  npm `openai` client's own tool loop (`runTools`), the TS `tool({...})`
  becomes `FunctionTool(...)` (marked) with an explicit `name = "chart"`
  (the TS tool's key in `tools: { chart }`), `inputSchema: Quarter` becomes
  `parameters = Quarter.schema, parse = Quarter.parse`, and
  `stopWhen: stepCountIs(5)` becomes `RunnerOptions(maxChatCompletions =
  5)`. The tool function receives the validated quarter string itself (TS
  destructures `{ quarter: q }`); the PNG is written in `.map` on
  `chartPng(q)` after `mkdirSync`, keeping the TS order. `.text` is
  `finalContent()`. `ask` has an explicit `@endpoint("POST", "/ask")`.
- artifacts/routes: TS validates `name` with the `Name` zod schema before
  the body runs; Scala tool parameters have no pattern constraint, so the
  body checks `Name.matches(name)` first and returns the declared usage
  error `Left(BadName.Invalid)` (TS `ok(...)` becomes `Right(...)`). The
  tool is the trait `Present` with `name = "present"`. Scala agents have no
  `init`, and `AppAgentImpl(chat)` takes the id as its constructor
  parameter.
- artifacts/files: TS's single `drawChart(await querySales(...))` line is
  `querySales(...).map: rows =>` plus the marked `val png =
  drawChart(rows)`; the return type marker sits on `def chart(...):
  Future[Png]` (after the two annotations), as the TS marks `returns: Png`.
  `Png` is a type alias like the TS `const Png`.
- artifacts/routes: the newly marked TS `methods: { sales: ... }` line is
  the marked `def sales(...) = querySales(region)` in `AppAgentImpl`.
- artifacts/login: the mount is unmarked and uncommented, as in TS, and
  `@agentDefinition(mount = ..., auth = true)` fits one line. The
  charts are a `private val ArrayBuffer[Chart]`, and the TS
  `if (made?.askedBy !== userKey(caller)) return undefined` is the marked
  `.filter(_.askedBy == userKey(caller))` on `charts.lift(n - 1)` (no
  chart, or another user's chart, gives `None` → 404). The TS comment
  "anyone else, even signed in, gets a 404:" sits above `charts.lift(n -
  1)`, the first line of that expression, one line above the marked
  `.filter`.
- agents/overview: the `class Id` line is no longer marked (TS `id:` line
  unmarked).
- tools/middleware: TS throws `ToolInvokeError`s; Scala handlers return
  `Future[Either[ToolInvokeError[E], A]]`, so refusals are failed Futures
  carrying the error (`refuse(why)` = `Future.failed(Refused(
  ConstraintViolation(why)))`, shown like the TS `refuse`) and one final
  `done.recover { case Refused(error) => Left(error) }` turns them into
  the handler's `Left`. `checkRateLimit` fails the same way. The sequence
  is a `for` over Futures; the TS `if (amount > 10_000) { ... }` is an
  `if ... then Future.unit else` generator holding the nested promise
  `for`. TS's `await underlying.refund(args)` throws on a tool error; Scala
  keeps the underlying's `Either` (`forwarded`), audits only a `Right`
  (`forwarded.fold(_ => Future.unit, r => audit(...))`) and redacts with
  `forwarded.map(_.copy(customerEmail = "[redacted]"))`, so the tool's own
  errors pass through unchanged. TS reads `principal` from the middleware
  context; the Scala typed middleware receives it only as a parameter the
  tool declares (`caller: Principal`, injected by Golem, not part of the
  tool's input), so the harness refund tool declares it (in both
  tools/middleware and tools/secrets, so the two stay identical).
  The `@toolMiddleware` line is the counterpart of the marked TS
  `refund.middleware({` line.
- artifacts/peruser: Scala agents have no `init`; the marked init
  counterpart is the constructor line `final class
  DashboardAgentImpl(user: String) extends DashboardAgent:` (the id is
  the constructor parameter; `made` is a `private var` initialised to
  0 on the next line). The TS init comment sits above
  `@agentImplementation()`. `++this.made` is `made += 1; val n = made`, and
  the write happens in `.map` on `dashboardFor(...)` after `mkdirSync`.
- artifacts/routers: TS matches the regex, then returns 404 first; Scala
  pattern-matches `(request.method, request.path)` (marked, the
  counterpart of `request.path.match(path)`) with `case ("GET",
  Csv(chat, id))` first and the 404 as `case _ =>` (marked, the
  counterpart of the TS `if`) last. TS's prelude `header(...)` helper is
  the SDK's own `HttpHeader.ascii`; TS `empty()` is a prelude `empty()`.
  The marked `body: csv` is the marked `), csv)` closing the response.
- artifacts/signin: the TS early `throw` for non-OIDC callers is the last
  match arm `case _ => Future.failed(Exception("sign-in required"))`; the
  marked TS destructuring `const { issuer, sub } = caller` is the marked
  `val (issuer, sub) = (user.issuer, user.sub)` inside `case user:
  Principal.Oidc =>`. The TS input marker (`caller: s.principal()`) is on
  the `def ask(question: String, caller: Principal)` signature; the
  comment above it sits between `@endpoint(...)` and `def`. TS
  `pageFor` returns a promise, so the page is built in `.flatMap`.

## Artifacts and middleware: no SDK gap, no shim

All seven artifacts snippets and tools/middleware compile against the
published SDK as written: the live-file mapping, query-variable endpoints,
`@description` with an endpoint, `UnstructuredBinary["image/png"]` (plain,
and in `Option`), `auth = true` with `Principal` injection, a
`requiresFilesystem` tool with a usage error enum, a raw `@httpRouter`
calling an agent's generated client that returns a byte stream, and a typed
`@toolMiddleware` using host promises. The only non-Golem pieces are the `openai`
facade (real npm client) and the `node:fs` facade.

## Secrets: no SDK bug of the TypeScript kind (read source and compiled)

- `golem.config.Secret[T].handle()` (`sdks/scala/model/src/main/scala/golem/config/Secret.scala:29-30`)
  returns a fresh owned `GuestSecretHandle`, loaded for configured secrets by
  `ConfigLoader.loadSecretHandleWire`
  (`sdks/scala/core/js/src/main/scala/golem/config/ConfigLoader.scala:99-111`)
  from `AgentHostApi.getConfigValue` as a real `SecretValue` handle.
- The generated tool client types the tool's secret positional as
  `apiKey: GuestSecretHandle` and encodes it with
  `ConcreteCodec[GuestSecretHandle]` (`SecretValue`,
  `sdks/scala/model/src/main/scala/golem/schema/GuestSecretHandle.scala:82-91`),
  so passing a `String` (or `Secret[String]`) fails to compile (neg control
  `tools-secrets`). No shim is needed.
- Not verified at runtime (no deployment).

## Negative controls (`neg.sh`)

`neg.sh [key ...]` copies `website/snippets` to a temp directory, applies one
mutation per snippet (an exact-text replacement that must match), runs
`check.sh` with `SNIPPETS_ROOT=<temp>`, and requires a non-zero sbt exit code
with a compiler error located in that snippet's spliced file. It then reruns
`check.sh` on the real files to restore `src/`. About 30 s per case.

Result of the recorded run (Oct 22 2026, 7 + 7 + 7 round): all 20 fail as expected, `NEG_OK`; the
restore run ends `ALL_OK`. "Located in" is where the compiler reports it:
Golem macro validations fire where the codegen expands the macro (the
generated auto-registration or RPC client for that snippet's package), which
`neg.sh` accepts alongside the spliced file.

| Snippet | Mutation | Expected error (located in) |
|---|---|---|
| agents/models | `DraftAgentImpl(topic: String)` → `(topic: Int)` | Golem macro: constructor parameter type differs from the Id class (generated auto-registration) |
| agents/overview | `class Id(val inbox: String)` → `(val inbox: Int)` | Golem macro: Id/constructor parameter type mismatch (generated auto-registration) |
| agents/team | `a.methods.map(_.name)` → `_.title` | E008 not a member of `golem.reflection.AgentMethod` (spliced file) |
| agents/waits | `.remind.scheduleCancelableAt(...)` → `.remind.scheduleAt(...)` | `scheduleAt` returns no cancellation token, so `reminder.cancel()` fails (spliced file) |
| agents/readonly | `@readOnly` → `@readOnly(true)` | E007: `cache` is a `String` policy (spliced file) |
| agents/stream | `AgentStream.fromPull:` → `AgentStream.of:` | E008 not a member of `golem.schema.AgentStream` (spliced file) |
| artifacts/files | `fromInline(png, "image/png")` → `"image/jpeg"` | E007: the MIME allow-list `"image/png"` is part of the type (spliced file) |
| artifacts/login | `@endpoint("GET", "/charts/{n}")` → `"/charts/{id}"` | Golem macro: path variable `id` is not a method parameter (generated auto-registration) |
| artifacts/overview | `mount = "/chats/{chat}"` → `"/chats/{room}"` | Golem macro: mount path variable not defined in the constructor (generated auto-registration) |
| artifacts/routes | `@endpoint("GET", "/sales?region={region}")` → `"/sales?area={area}"` | Golem macro: query variable `area` is not a method parameter (generated auto-registration) |
| tools/custom | `OutlineClient().outline(file)` → `outline(path = file)` | generated `OutlineClient.outline(file: String)` has no `path` parameter (spliced file) |
| tools/overview | `SummarizeClient().summarize(url)` → `summarize(uri = url)` | generated `SummarizeClient.summarize(url: String)` has no `uri` parameter (spliced file) |
| tools/permissions | `Some(PermissionsApi.Lifespan.iso8601("PT1H"))` → `"PT1H"` | E007: required `Option[PermissionsApi.Lifespan]` (spliced file) |
| tools/publish | `call.collect()` → `call.drain()` | E008 not a member of `golem.tool.ToolInvocation` (spliced file) |
| tools/requests | `HttpMethod.POST` → `HttpMethod.SEND` | E008 not a member of `org.scalajs.dom.HttpMethod` (spliced file); this snippet uses no Golem API |
| tools/secrets | `config.value.stripeKey.handle()` → `config.value.stripeKey.get` | E007: found `String`, required `golem.schema.GuestSecretHandle` (spliced file) |
| tools/middleware | `underlying.refund(charge, amount, apiKey)` → `underlying.refund(charge, amount, apiKey, caller)` | too many arguments for generated `RefundToolUnderlying.refund(charge, amount, apiKey)`: the principal is injected, never forwarded (spliced file) |
| artifacts/peruser | `mount = "/dashboards/{user}"` → `"/dashboards/{name}"` | Golem macro: mount path variable `name` is not defined in the agent constructor (generated auto-registration) |
| artifacts/routers | `HttpHeader.ascii("content-type", "text/csv")` → `HttpHeader("content-type", "text/csv")` | E007: found `String`, required `Array[Byte]` (header values are bytes) (spliced file) |
| artifacts/signin | `case user: Principal.Oidc =>` → `case user: Principal.GolemUser =>` | E008: `issuer` / `sub` are not members of `golem.Principal.GolemUser` (spliced file) |

Two candidate controls compiled and were dropped: removing `derives Schema`
from `Draft` (the Golem macro derives the schema itself) and removing
`extends BaseAgent` from `DraftAgent` (not required by the macros).

## Known limits (compiles and links; not proven at runtime)

- Nothing was deployed or executed: the check proves type-correctness
  against the SDK macros, codegen and Scala.js linker only. Some metadata
  rules may only be enforced when the component registers with the host.
- The chart tool loop (`runTools`) is type-checked against the hand-written
  facade only; the facade's shape follows the openai-node docs, not a
  compiled `.d.ts` (no npm install in this harness).
- `ai`/`aisdk`, the bash client and the crm_find_customer client are type-only and throw at runtime; the
  generated bash client's exact shape for the current bash release (0.3.0,
  which adds an optional `stdin` stream) is not checked, only the documented
  three-argument call.
- In production `ResearchAgentClient` and `SlackAgentClient` come from the
  guest bridge generator, not from the in-component codegen used here.
- The `openai` npm module (and `node:timers/promises`) are assumed to be
  available to Scala.js components in the Golem QuickJS runtime, as for
  TypeScript; `openai` would have to be bundled.
- Durable sleep relies on the QuickJS runtime's timers being durable, as the
  TS snippet does; the Scala SDK documents no sleep API of its own.
- Webhook delivery, reminder cancellation, permission lifetimes, secret
  reveal grants, streaming resumption, read-only caching, CORS/auth
  enforcement and file serving are not exercised.

## Re-running

```shell
.amp/in/snippets/scala/check.sh   # ends with ALL_OK (about 20–30 s warm)
.amp/in/snippets/scala/neg.sh     # 20 controls, ends with NEG_OK (about 10 min)
```

Re-publish the SDK first (see above) if `sdks/scala` changed since the last
local publication.
