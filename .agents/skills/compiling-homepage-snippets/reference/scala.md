# Scala home-page snippets: how they are compiled

This directory proves that every Scala per-feature display snippet on the home
page compiles and links against the in-tree Golem Scala.js SDK (`sdks/scala`),
including the Golem macros (agent/tool/HTTP metadata validation) and the Golem
sbt codegen (RPC and tool clients, auto-registration). Verified at repository
commit `d35a841e6` (with uncommitted snippet edits; re-verified Oct 22 2026
after the chapter reorganization: durable streaming and read-only methods
moved to Agents, "Your language" dropped, new TypeScript designs ported,
and `tools/publish` became a code snippet) on macOS arm64.

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
| tools | publish | `website/snippets/tools/publish/scala/AccountAgent.scala` |
| artifacts | overview | `website/snippets/artifacts/overview/scala/BriefAgent.scala` |
| artifacts | routes | `website/snippets/artifacts/routes/scala/BoardAgent.scala` |
| artifacts | routers | `website/snippets/artifacts/routers/scala/ExportsRouter.scala` |
| artifacts | login | `website/snippets/artifacts/login/scala/PayslipAgent.scala` |

There is deliberately no `agents/sqlite/scala` (Scala agents have no embedded
SQLite; the page shows the chapter's `missing.sqlite` note), so the Scala set
is the 17 TypeScript features minus sqlite: 16 files. The Scala names match
the TypeScript agent/tool names (`TriageAgent`, `StockAgent`, tool
`summarize` + `DigestAgent`, `AccountAgent`, `BriefAgent`, `BoardAgent`,
`ExportsRouter`, `PayslipAgent`, ...).

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
    lib/openai.scala        facade over the npm `openai` client (see below)
    lib/ai.scala            invented `ai` library (Vercel AI SDK port), fails at runtime
    lib/aisdk.scala         invented `aisdk.openai` / `aisdk.anthropic` providers
    lib/node.scala          `node.fs` (node:fs, text and Uint8Array reads) and `node.sleep`
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
   (`*/*/ts/*.ts`, 17 features) minus `agents/sqlite`, with exactly one
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
| `openai.{OpenAI, ChatParams, ChatMessage, responseFormat, ResponseParams}` | `harness/lib/openai.scala` | hand-written Scala.js facade over the real npm `openai` client (reused from the previous harness, extended) | agents/overview uses `openai.chat.completions.parse(...)` and `message.parsed`; agents/stream uses `openai.responses.create(...)` and `output_text`, as the TS snippets do. `responseFormat[A](name)` is the analogue of `zodResponseFormat`, really implemented from `Schema[A].toJsonSchema`. Imported as `_root_.openai.{...}` because the snippets' client value is also called `openai`. |
| `node.fs` | `harness/lib/node.scala` | facade over `node:fs` (provided by the Golem QuickJS runtime; the approach documented in `golem-skills/skills/scala/golem-file-io-scala`) | `readFileSync` (text, and the documented `Uint8Array` overload used by artifacts/login), `writeFileSync`, `mkdirSync`. Snippets pass `literal(recursive = true)` (`scala.scalajs.js.Dynamic.literal`, imported by the prelude). |
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
`MailerAgent` (agents/waits), and `ExportAgent` (artifacts/routers; id
`class Id(val id: String)`, `def csv(): AgentStream[Array[Byte]]`, the
TypeScript `s.stream(s.bytes())`), and the refund tool `RefundTool` that
tools/secrets calls (`harness/extra/tools-secrets.scala`; `Refund` and
`stripeRefund` are in that snippet's prelude, since the refund tool is no
longer a display snippet). Both tools/overview and tools/secrets are
self-contained packages.

Prelude helpers shown without definition, as in the TS preludes: `classify`
(returns the label name, parsed with `Label.valueOf` as TS uses
`Label.parse`) and `applyLabel` (agents/overview); `Facts`, `Plan` (team);
`PaymentEvent`, `inDays`, `createPaymentLink`, `sendSurvey`, `sleep`
(waits); `openai` (artifacts overview, stream); `Headings` (custom);
`summarizeText` and `Urls` (tools overview); `AgentId` =
`HostApi.AgentIdLiteral` (permissions); `orThrow` (secrets, custom, team,
permissions, tools overview, publish). The routers prelude only aliases
`AgentStream`; the display builds headers with the native
`HttpHeader(name, Array[Byte])` and its own `bytes` helper, and the empty
404 body with `AgentStream.fromStream(Stream.empty)`, as the TypeScript now
does with plain records.

## Real Golem Scala SDK APIs used

`@agentDefinition(mount, auth, exposeFiles)` (overview: `/` →
`/out/index.html`; routes: `/app` → `/index.html`, written by the agent's
constructor via `node:fs`), `@agentImplementation`,
`@endpoint(method, path)`, `@readOnly` (on a method without an HTTP
endpoint), `@description` on an agent method (tools/publish),
`golem.schema.UnstructuredBinary["application/pdf"]` returned as
`Option[...]` from a GET endpoint (raw body, `None` → 404; see
`sdks/scala/docs/unstructured.md`),
`golem.tool.ToolInvocation.collect()`,
Scala 3 `enum ... derives Schema` and `Map[String, Label]` results, `@httpRouter(typeName, mount)` with `@httpHandler def serve(request: HttpRequest):
Future[HttpResponse]` (the raw handler; `golem.runtime.http.{HttpRequest,
HttpResponse, HttpHeader}` (`HttpHeader(name, bytes)`), `UShort` status), the generated
`ExportAgentClient.get(id).csv()` returning
`Future[AgentStream[Array[Byte]]]` relayed as the response body,
`golem.Principal` / `Principal.Oidc` method injection,
`golem.schema.AgentStream.fromPull` (synchronous `AgentStream[String]`
returns), `HostApi.createWebhook()` with `.url` and
`.await().map(_.json[A]())`, generated clients `XClient.get(...)`, awaited
calls, `.trigger(...)`, `.scheduleCancelableAt(when)`
(→ `CancellationToken.cancel()`), `golem.Datetime.afterSeconds`,
`golem.reflection.Reflection.getAllAgentTypes()` (`name`, `methods.map(_.name)`)
and `getAllToolTypes()` (`name`), `@toolDefinition(name, version,
requiresFilesystem)`, `@toolImplementation`, `@error(kind, exitCode)` on a
tool error enum, generated tool clients `OutlineClient()` /
`SummarizeClient()` / `RefundToolClient()` (typed `apiKey: GuestSecretHandle`),
`golem.schema.GuestSecretHandle` +
`golem.host.SecretApi.reveal[String]`, `golem.config.{AgentConfig, Config,
Secret}` (`AgentConfig[T]` on the trait, `Config[T]` constructor
parameter, `Secret[String].handle()` for an owned handle without revealing),
and `golem.host.PermissionsApi.{PatternGrant, Bounds,
deriveFromWallet, PersistentCard.derive/install/revoke, Lifespan.iso8601,
Holder.Agent}`.

## Scala-specific workarounds

- Generated RPC clients copy agent-method signatures but not the source
  file's imports, so `AgentStream` is aliased at package level in the
  artifacts overview/stream/routers preludes (`type AgentStream[+A] =
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
- artifacts/stream: TS async generators become
  `AgentStream.fromPull(() => Future[Option[A]])`.
- artifacts/routes: the TS `init` becomes the `BoardAgentImpl` constructor
  body (Scala agents have no separate init hook). The page string is a
  triple-quoted literal byte-identical to the TS template literal.
- artifacts/routers: the TS regex match plus `if (method !== 'GET' ||
  !id) return 404` becomes a pattern match on `(request.method,
  request.path)` with a regex extractor `Csv`, so the 404 case comes last
  (`case _`). The generated client returns
  `Future[AgentStream[Array[Byte]]]`, so the response is built in `.map`.
  No shim: the SDK relays another agent's stream
  (`sdks/scala/model/src/main/scala/golem/schema/AgentStream.scala:446-455`).
- artifacts/login: the TS tag/subject check becomes a `Principal` pattern
  match with a guard; `fs.readFileSync` returns a `Uint8Array`, converted
  with `pdf.toArray.map(_.toByte)` because `UnstructuredBinary.Inline`
  holds an `Array[Byte]`.

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

Result of the recorded run (Oct 22 2026): all 16 fail as expected, `NEG_OK`; the
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
| artifacts/login | `fromInline(data, "application/pdf")` → `"text/html"` | E007: the MIME allow-list `"application/pdf"` is part of the type (spliced file) |
| artifacts/overview | `mount = "/briefs/{client}"` → `"/briefs/{brief}"` | Golem macro: mount path variable not defined in the constructor (generated code) |
| artifacts/routers | `HttpHeader("content-type", bytes("text/csv"))` → `HttpHeader("content-type", "text/csv")` | E007: header values are `Array[Byte]` (spliced file) |
| artifacts/routes | `@endpoint("POST", "/cards")` → `"/cards/{id}"` | Golem macro: path variable `id` is not a method parameter (generated code) |
| tools/custom | `OutlineClient().outline(file)` → `outline(path = file)` | generated `OutlineClient.outline(file: String)` has no `path` parameter (spliced file) |
| tools/overview | `SummarizeClient().summarize(url)` → `summarize(uri = url)` | generated `SummarizeClient.summarize(url: String)` has no `uri` parameter (spliced file) |
| tools/permissions | `Some(PermissionsApi.Lifespan.iso8601("PT1H"))` → `"PT1H"` | E007: required `Option[PermissionsApi.Lifespan]` (spliced file) |
| tools/publish | `call.collect()` → `call.drain()` | E008 not a member of `golem.tool.ToolInvocation` (spliced file) |
| tools/requests | `HttpMethod.POST` → `HttpMethod.SEND` | E008 not a member of `org.scalajs.dom.HttpMethod` (spliced file); this snippet uses no Golem API |
| tools/secrets | `config.value.stripeKey.handle()` → `config.value.stripeKey.get` | E007: found `String`, required `golem.schema.GuestSecretHandle` (spliced file) |

Two candidate controls compiled and were dropped: removing `derives Schema`
from `Draft` (the Golem macro derives the schema itself) and removing
`extends BaseAgent` from `DraftAgent` (not required by the macros).

## Known limits (compiles and links; not proven at runtime)

- Nothing was deployed or executed: the check proves type-correctness
  against the SDK macros, codegen and Scala.js linker only. Some metadata
  rules may only be enforced when the component registers with the host.
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
.amp/in/snippets/scala/neg.sh     # 16 controls, ends with NEG_OK (about 8 min)
```

Re-publish the SDK first (see above) if `sdks/scala` changed since the last
local publication.
