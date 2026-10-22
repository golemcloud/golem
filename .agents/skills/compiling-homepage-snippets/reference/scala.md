# Scala home-page snippets: how they are compiled

This directory proves that every Scala per-feature display snippet on the home
page compiles and links against the in-tree Golem Scala.js SDK (`sdks/scala`),
including the Golem macros (agent/tool/HTTP metadata validation) and the Golem
sbt codegen (RPC and tool clients, auto-registration). Verified at repository
commit `a2ec4edf3` (with uncommitted snippet edits; re-verified Oct 21 2026
after the artifacts/routes rewrite and the new artifacts/routers feature) on macOS arm64.

## Display files (the only inputs; read in place, never copied by hand)

`check.sh` discovers every file matching `website/snippets/*/*/scala/*`:

| Chapter | Feature | File |
|---|---|---|
| agents | overview | `website/snippets/agents/overview/scala/SupportAgent.scala` |
| agents | language | `website/snippets/agents/language/scala/CartAgent.scala` |
| agents | models | `website/snippets/agents/models/scala/DraftAgent.scala` |
| agents | team | `website/snippets/agents/team/scala/PlannerAgent.scala` |
| agents | waits | `website/snippets/agents/waits/scala/CheckoutAgent.scala` |
| tools | overview | `website/snippets/tools/overview/scala/RefundTool.scala` |
| tools | custom | `website/snippets/tools/custom/scala/BuildAgent.scala` |
| tools | requests | `website/snippets/tools/requests/scala/StripeRefund.scala` |
| tools | permissions | `website/snippets/tools/permissions/scala/OrdersAccess.scala` |
| tools | secrets | `website/snippets/tools/secrets/scala/SupportAgent.scala` |
| artifacts | overview | `website/snippets/artifacts/overview/scala/SiteAgent.scala` |
| artifacts | routes | `website/snippets/artifacts/routes/scala/ReportAgent.scala` |
| artifacts | routers | `website/snippets/artifacts/routers/scala/ExportsRouter.scala` |
| artifacts | stream | `website/snippets/artifacts/stream/scala/WriterAgent.scala` |
| artifacts | login | `website/snippets/artifacts/login/scala/NotebookAgent.scala` |
| artifacts | readonly | `website/snippets/artifacts/readonly/scala/PageAgent.scala` |

There is deliberately no `agents/sqlite/scala` (Scala agents have no embedded
SQLite; the page shows the chapter's `missing.sqlite` note), and the shared
`tools/publish/golem.yaml` is not ported. The Scala names match the
TypeScript: agent `SupportAgent`, tool `refund`
(`@toolDefinition(name = "refund", version = "1.0.0")`), secret config field
`stripeKey`, method `issueRefund`.

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
    lib/node.scala          `node.fs` (node:fs) and `node.sleep` (node:timers/promises) facades
    lib/bash.scala          stub of the generated bash tool client (golem.bridge.client.bash)
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
   (`*/*/ts/*.ts`) minus `agents/sqlite`, with exactly one Scala file per
   feature and no `agents/sqlite/scala`.
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
| `openai.{OpenAI, ChatParams, ChatMessage, responseFormat, ResponseParams}` | `harness/lib/openai.scala` | hand-written Scala.js facade over the real npm `openai` client (reused from the previous harness, extended) | agents/overview uses `openai.chat.completions.parse(...)` and `message.parsed`; artifacts/stream uses `openai.responses.create(...)` and `output_text`, as the TS snippets do. `responseFormat[A](name)` is the analogue of `zodResponseFormat`, really implemented from `Schema[A].toJsonSchema`. Imported as `_root_.openai.{...}` because the snippets' client value is also called `openai`. |
| `node.fs` | `harness/lib/node.scala` | facade over `node:fs` (provided by the Golem QuickJS runtime; the approach documented in `golem-skills/skills/scala/golem-file-io-scala`) | `readFileSync`, `writeFileSync`, `mkdirSync`. Snippets pass `literal(recursive = true)` (`scala.scalajs.js.Dynamic.literal`, imported by the prelude). |
| `node.sleep` | `harness/lib/node.scala` | facade over `setTimeout` from `node:timers/promises`, returned as a `Future` | the same durable sleep the TS waits snippet imports; the Scala SDK has no sleep API of its own. |
| `golem.bridge.client.bash.{BashClient, BashResult, BashError}` | `harness/lib/bash.scala` | stub of a generated client; throws at runtime | stands in for the client `golem build` generates for the built-in bash tool (`golem-temp/bridge-sdk/scala/internal/bash-tool-guest-client`). Shape from `cli/golem-cli/src/bridge_gen/scala/tool.rs` (`BashClient()` / `newClient()`, `Future[Either[ToolError[E], A]]`, u8 → `golem.UByte`, u32 → `golem.UInt`), the call in `golem-call-tool-scala` (`run(script, cwd, UInt(600))`) and the result/error types in `builtin-tools/bash/component/src/lib.rs`. |
| `homepage.orThrow` | `harness/lib/support.scala` | helper | unwraps `Either` results (`PermissionError`, `GolemReflectError`, `ToolError`) or throws, like TS `.unwrap()`; shown undefined in the snippets. |

Other agents a snippet calls are real Scala agents in `harness/extra/`, so
the Golem codegen generates their clients exactly as for any agent:
`ResearchAgent` and `SlackAgent` (agents/team; in production ResearchAgent is
a Rust agent whose guest bridge client, generated by
`cli/golem-cli/src/bridge_gen/scala/mod.rs`, has the same `get`, awaited
`apply`, `trigger`, `scheduleAt` and `scheduleCancelableAt` surface),
`MailerAgent` (agents/waits), and `ExportAgent` (artifacts/routers; id
`class Id(val id: String)`, `def csv(): AgentStream[Array[Byte]]`, the
TypeScript `s.stream(s.bytes())`). tools/secrets calls the refund tool of the
tools/overview snippet: its prelude imports the generated
`check.tools.overview.RefundToolClient` and aliases
`type Refund = check.tools.overview.Refund` at package level (generated RPC
clients do not copy imports). Both `agents/overview` and `tools/secrets`
define an agent named `SupportAgent` (different packages); this compiles and
links, but a real component would hold only one of them.

Prelude helpers shown without definition, as in the TS preludes: `placeOrder`
(language); `Message`, `Reply`/`Reply.parse`, `Approval`, `notifyReviewer`,
`refundOrder`, `openai` (overview); `Facts`, `Plan`, `inDays` (team);
`PaymentEvent`, `inDays`, `createPaymentLink`, `sendSurvey`, `sleep`
(waits); `streamPage` (artifacts overview); `frontEnd`, `render`
(routes); `empty()` (routers: an empty `AgentStream[Array[Byte]]`, as in
the TS prelude); `Headings`
(custom); `Refund`/`Refund.parse` and `stripeRefund` (tools overview);
`AgentId` = `HostApi.AgentIdLiteral` (permissions); `orThrow` (secrets,
custom, team, permissions).

## Real Golem Scala SDK APIs used

`@agentDefinition(mount, auth, cors, exposeFiles)` (routes: `/` and a
`/*` → `/site/$1` subtree, written by the agent's constructor via
`node:fs`), `@agentImplementation`,
`@endpoint(method, path)`, `@readOnly`, `@httpRouter(typeName, mount)` with `@httpHandler def serve(request: HttpRequest):
Future[HttpResponse]` (the raw handler; `golem.runtime.http.{HttpRequest,
HttpResponse, HttpHeader.ascii}`, `UShort` status), the generated
`ExportAgentClient.get(id).csv()` returning
`Future[AgentStream[Array[Byte]]]` relayed as the response body,
`golem.Principal` / `Principal.Oidc` method injection,
`golem.schema.AgentStream.fromPull` (synchronous `AgentStream[String]`
returns), `HostApi.createWebhook()` with `.url` and
`.await().map(_.json[A]())`, generated clients `XClient.get(...)`, awaited
calls, `.trigger(...)`, `.scheduleAt(when)`, `.scheduleCancelableAt(when)`
(→ `CancellationToken.cancel()`), `golem.Datetime.afterSeconds`,
`golem.reflection.Reflection.getAllAgentTypes()` (`name`, `sourceLanguage`)
and `getAllToolTypes()` (`name`), `@toolDefinition(name, version,
requiresFilesystem)`, `@toolImplementation`, `@error(kind, exitCode)` on a
tool error enum, generated tool clients `OutlineClient()` /
`RefundToolClient()` (typed `apiKey: GuestSecretHandle`),
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

- agents/overview: Scala `Future` code has no early `return`, so the
  refund branch is a private `review(orderId, reply)` method (as in the
  previous Scala design); this also keeps the comment lines within 68
  characters.
- agents/waits: the wait chain is a `for` over `Future`s; the
  "not succeeded" early return becomes `if ... then Future.unit else`.
  `sleep` is the `node:timers/promises` facade (prelude), not an import.
- agents/team: `Reflection.getAll*Types()` return `Either`, unwrapped with
  `orThrow`; `trigger`/`scheduleAt` return `Future`s that are discarded, as
  TS discards its results.
- tools/overview: the macro kebab-cases parameter names, so the tool's
  positional is projected as `api-key`; TS names it `apiKey`. `charge` and
  `amount` are identical. Reveal is `SecretApi.reveal[String](apiKey)`
  (typed), not `stringValue(reveal(apiKey, stringType))`.
- tools/custom: the generated tool clients return `Either`s, unwrapped
  with `orThrow`; bash's `exitCode` is a `golem.UByte` (`.value`).
- artifacts/overview and stream: TS async generators become
  `AgentStream.fromPull(() => Future[Option[A]])`.
- artifacts/login: the TS tag check becomes a `Principal` pattern match.
- artifacts/routes: the TS `init` becomes the `ReportAgentImpl`
  constructor body (Scala agents have no separate init hook); the team id
  is the constructor parameter `team`. `build` maps over `render(month)`
  instead of `await`.
- artifacts/routers: the TS regex match plus `if (method !== 'GET' ||
  !match) return 404` becomes a pattern match on `(request.method,
  request.path)` with a regex extractor `Csv` (a `private val` on the
  router class), so the 404 case comes last (`case _`) instead of first.
  The highlights follow: the regex line, the client call, the
  content-disposition header, the `rows` body, and the `case _` + 404
  lines (the TS condition line's counterpart is `case _`). TS `header(...)`
  (prelude helper) is the SDK's own `HttpHeader.ascii`. The generated
  client returns `Future[AgentStream[Array[Byte]]]`, so the response is
  built in `.map` rather than after `await`. No shim: the SDK relays
  another agent's stream (`AgentStream.concreteCodec.write` moves the
  decoded stream to the wire, with a direct handle transfer path,
  `sdks/scala/model/src/main/scala/golem/schema/AgentStream.scala:446-455`).
  Not verified at runtime.
- artifacts/overview: the `fromPull` body uses braces
  (`AgentStream.fromPull(() => parts.pull().map { ... })`) so the
  write-file comment fits in 68 columns at its TypeScript position.
- artifacts/readonly: the three comments precede `@readOnly`, which
  precedes `@endpoint("GET", "/")`, matching the TS order (readOnly, http).
- tools/secrets: the config is a case class (`SupportConfig`) mixed in via
  `AgentConfig[SupportConfig]`, so the comment and the marked line sit on
  that case class. The secret goes to the tool as
  `config.value.stripeKey.handle()`; the tool client returns
  `Future[Either[ToolError[RefundError], Refund]]`, unwrapped with
  `.map(_.orThrow)`.

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

Result of the recorded run: all 16 fail as expected, `NEG_OK`; the
restore run ends `ALL_OK`. "Located in" is where the compiler reports it:
Golem macro validations fire where the codegen expands the macro (the
generated auto-registration or RPC client for that snippet's package), which
`neg.sh` accepts alongside the spliced file.

| Snippet | Mutation | Error (located in) |
|---|---|---|
| agents/language | `@agentDefinition()` → `@agentDefinition(mount = "/carts/{cart}")` | Golem macro: HTTP mount path variable 'cart' is not defined in the agent constructor (generated `__GolemAutoRegister_check_agents_language`) |
| agents/models | `DraftAgentImpl(topic: String)` → `(topic: Int)` | Golem macro: constructor parameter 'topic' has type Int, but Id class expects String (generated auto-registration) |
| agents/overview | `approval.url,` → `approval.url(),` | E171: `url` is a `String` field of `WebhookHandler`, not a method (spliced file) |
| agents/team | `Reflection.getAllToolTypes()` → `getAllTools()` | E008 not a member of `golem.reflection.Reflection` (spliced file) |
| agents/waits | `reminder.cancel()` → `reminder.abort()` | E008 not a member of `golem.runtime.rpc.CancellationToken` (spliced file) |
| artifacts/login | `login.sub` → `login.subject` | E008 not a member of `golem.Principal.Oidc` (spliced file) |
| artifacts/overview | `mount = "/sites/{project}"` → `"/sites/{site}"` | Golem macro: mount path variable 'site' not defined in the agent constructor (generated auto-registration and `SiteAgentClient`) |
| artifacts/readonly | `@readOnly` → `@readOnly(true)` | E007: `cache` is a `String` policy (spliced file) |
| artifacts/routes | `("/*", "/site/$1")` → `("/*", "/site/$2")` | Golem macro: `exposeFiles: target-placeholder` (`$2` has no matching route wildcard; generated auto-registration) |
| artifacts/routers | `@httpRouter("ExportsRouter", "/exports")` → `"/exports/{id}"` | Golem macro: `router-mount: source-path` (router mounts must be literal; generated auto-registration) |
| artifacts/stream | `AgentStream.fromPull:` → `AgentStream.of:` | E008 not a member of `golem.schema.AgentStream` (spliced file) |
| tools/custom | `OutlineClient().outline(file)` → `outline(path = file)` | generated `OutlineClient.outline(file: String)` has no `path` parameter (spliced file) |
| tools/overview | `SecretApi.reveal[String](apiKey)` → `(charge)` | E007: found `String`, required `golem.schema.GuestSecretHandle` (spliced file) |
| tools/permissions | `Some(PermissionsApi.Lifespan.iso8601("PT1H"))` → `"PT1H"` | E007: required `Option[PermissionsApi.Lifespan]` (spliced file) |
| tools/secrets | `config.value.stripeKey.handle()` → `config.value.stripeKey.get` | E007: found `String`, required `golem.schema.GuestSecretHandle` (spliced file) |
| tools/requests | `HttpMethod.POST` → `HttpMethod.SEND` | E008 not a member of `org.scalajs.dom.HttpMethod` (spliced file). This snippet uses no Golem API (Golem's permission check and idempotency key apply to plain `fetch`), so the control targets the request API it does use. |

Two candidate controls compiled and were dropped: removing `derives Schema`
from `Draft` (the Golem macro derives the schema itself) and removing
`extends BaseAgent` from `DraftAgent` (not required by the macros). Changing
the `CartAgentImpl` constructor type failed only through an ordinary type
error in the body, so agents/language uses the mount control instead.

## Known limits (compiles and links; not proven at runtime)

- Nothing was deployed or executed: the check proves type-correctness
  against the SDK macros, codegen and Scala.js linker only. Some metadata
  rules may only be enforced when the component registers with the host.
- `ai`/`aisdk` and the bash client are type-only and throw at runtime; the
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
