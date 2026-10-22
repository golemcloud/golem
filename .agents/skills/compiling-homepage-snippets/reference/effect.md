# Effect home-page snippets: how they are compiled

This harness type-checks every Effect per-feature display snippet against the in-tree Effect SDK
(`sdks/effect`, package `@golemcloud/effect-golem`) with the SDK's own TypeScript compiler. Effect
components are TypeScript that Rollup bundles into a QuickJS-backed WASM component, so a strict
`tsc --noEmit` against the SDK's published declarations is the compile step that can fail on API
misuse. No `.wasm` is produced (see "Known limits").

## Display files (inputs, read in place)

`check.sh` finds every file matching `website/snippets/<chapter>/<feature>/effect/<file>`
(exactly four path levels below the snippet root). Effect keeps the TypeScript file names:

| Chapter / feature | File |
|---|---|
| agents/overview | `website/snippets/agents/overview/effect/triage-agent.ts` |
| agents/sqlite | `website/snippets/agents/sqlite/effect/notes-agent.ts` |
| agents/models | `website/snippets/agents/models/effect/draft-agent.ts` |
| agents/team | `website/snippets/agents/team/effect/planner-agent.ts` |
| agents/waits | `website/snippets/agents/waits/effect/checkout-agent.ts` |
| tools/overview | `website/snippets/tools/overview/effect/digest-agent.ts` |
| tools/custom | `website/snippets/tools/custom/effect/build-agent.ts` |
| tools/requests | `website/snippets/tools/requests/effect/stripe-refund.ts` |
| tools/permissions | `website/snippets/tools/permissions/effect/orders-access.ts` |
| tools/secrets | `website/snippets/tools/secrets/effect/support-agent.ts` |
| tools/publish | `website/snippets/tools/publish/effect/account-agent.ts` |
| artifacts/overview | `website/snippets/artifacts/overview/effect/analyst-agent.ts` |
| artifacts/routes | `website/snippets/artifacts/routes/effect/app-tool.ts` |
| artifacts/files | `website/snippets/artifacts/files/effect/chart-agent.ts` |
| agents/stream | `website/snippets/agents/stream/effect/writer-agent.ts` |
| artifacts/login | `website/snippets/artifacts/login/effect/private-chart-agent.ts` |
| agents/readonly | `website/snippets/agents/readonly/effect/stock-agent.ts` |
| tools/middleware | `website/snippets/tools/middleware/effect/refund-policy.ts` |
| artifacts/peruser | `website/snippets/artifacts/peruser/effect/dashboard-agent.ts` |
| artifacts/routers | `website/snippets/artifacts/routers/effect/export-router.ts` |
| artifacts/signin | `website/snippets/artifacts/signin/effect/billing-agent.ts` |

`tools/publish` is a per-language code snippet (the shared `golem.yaml` is gone). `tools/secrets` is a per-language snippet
(`SupportAgent`, secret config field `stripeKey`, tool `refund`, method `issueRefund`).

### Highlight markers

A display line ending in ` //@` (` #@` in YAML) is highlighted on the page while its snippet
shows; the page strips the marker. Mark the Effect line(s) that correspond to each marked
TypeScript line (same Golem operation or declaration; when one TS line becomes several Effect
lines, mark the one with the Golem call). Markers may also follow a comment line (the comment
must still end with `:` once the marker is stripped). `check.sh` strips markers before every
rule and compile step, so they never count toward width.

## Prerequisites (versions used)

- macOS arm64, Node `v26.0.0`, npm `11.14.1` (any Node ≥ 20 should do).
- The Effect SDK installed and built in the tree:

  ```sh
  cd sdks/effect
  npm ci          # installs effect@4.0.0, typescript@5.9.3, @types/node, rollup, ...
  npm run build   # produces dist/ (dist/effect-golem*.d.ts, dist/src/**/*.d.ts)
  ```

  `check.sh` needs only `sdks/effect/dist/` (declarations) and `sdks/effect/node_modules/`
  (`effect`, `typescript`, `@types/node`). It refuses to run without `dist/effect-golem.d.ts`.
  The checked-in ambient WIT declarations `sdks/effect/golem-types/*.d.ts` are added to every
  compilation so `golem:*` types are real, not `any`.
- Two real Effect AI providers from npm, at exactly the installed `effect` version (as
  `golem-skills/skills/effect/golem-add-llm-effect` requires), installed into the harness only:

  ```sh
  cd .amp/in/snippets/effect/vendor
  npm install --save-exact --legacy-peer-deps @effect/ai-openai@4.0.0 @effect/ai-anthropic@4.0.0
  ```

  `vendor/package.json` pins both; `check.sh` runs `npm install --legacy-peer-deps` there when
  either is missing. `--legacy-peer-deps` stops npm from installing a second `effect`: the
  providers must share the SDK's single `effect` instance or service tags (`HttpClient`,
  `LanguageModel`) do not unify. `check.sh` fails if either provider version differs from
  `effect`.

No environment variables are needed for checking.

## Harness layout (`.amp/in/snippets/effect/`)

```
check.sh                 style checks, splice, one tsc per snippet; prints ALL_OK
neg.sh                   one wrong-API mutation per snippet; prints NEG_OK
tsconfig.base.json       strict, NodeNext, noEmit, skipLibCheck, lib ES2022+DOM, types [node],
                         paths for the three generated-client stubs
prelude/_common.ts       postJson / postJsonFor (FetchHttpClient, trap on non-2xx), inDays(n): Date
prelude/<chapter>-<feature>.ts   imports + omitted supporting code for that snippet
prelude/tools-requests.post.ts   appended after the requests snippet: the refund tool using it
stubs/_guest.ts          GuestMethod / Get types copied from the Effect guest bridge generator
stubs/research-agent-guest-client.ts, stubs/slack-agent-guest-client.ts,
stubs/bash-tool-guest-client.ts,
stubs/crm-find-customer-tool-guest-client.ts  stand-ins for `golem build` generated clients
vendor/                  npm project holding @effect/ai-openai@4.0.0 and @effect/ai-anthropic@4.0.0
node_modules/            symlinks created by check.sh:
  effect                    -> sdks/effect/node_modules/effect
  @golemcloud/effect-golem  -> sdks/effect
  @types/node               -> sdks/effect/node_modules/@types/node
  @effect/ai-openai         -> vendor/node_modules/@effect/ai-openai
  @effect/ai-anthropic      -> vendor/node_modules/@effect/ai-anthropic
gen/                     generated sources, tsconfigs and tsc output (rebuilt on every run)
gen/sdk-fixed/           SHIM: copy of sdks/effect/dist with the Tool.ts:92 fix (tools-overview only)
```

TypeScript resolves symlinks to real paths. The providers' real paths are inside `vendor/`, so
their `import "effect"` walks up to `.amp/in/snippets/effect/node_modules/effect`, the same real
directory the SDK uses. That is why `vendor/` lives under the harness directory.

## What `check.sh [snippet-root]` does

Default root: `website/snippets`. Each display file is first copied to `gen/display/<rel>` with
a trailing highlight marker removed (`sed -E 's,[ ]+(//|#)@$,,'`); every later step reads only
that stripped copy.

1. Links the toolchain (above) and prints `effect`, provider and `tsc` versions.
2. Style, per display file: every line ≤ 68 columns, no tabs; after blanking `'…'`, `"…"` and
   `` `…` `` literals, every `//` comment must be a whole line ending with `:` and directly above
   code (no trailing or block comments); at most 31 lines. Prints
   `<chapter>/<feature>/effect/<file> lines=N max=M markers=K` for each (K counts markers in
   the original file).
3. Splice, per display file: `gen/<chapter>-<feature>.ts =
   prelude/<chapter>-<feature>.ts + blank line + stripped display file + prelude/<chapter>-<feature>.post.ts`
   (the post file only exists for `tools-requests`). `prelude/_common.ts` is copied to
   `gen/_common.ts`. Display files that show imports (`agents/models`, `agents/sqlite`) keep
   them; ES imports are hoisted, so they may follow prelude code.
4. Writes `gen/tsconfig.<chapter>-<feature>.json`
   (`extends ../tsconfig.base.json`, `files: [<f>.ts]`, `include: [sdks/effect/golem-types/*.d.ts]`)
   and runs, printing the command and exit code. For the snippets listed in `FIXED_SDK`
   (only `tools-overview`) it first builds `gen/sdk-fixed/` (a copy of `sdks/effect/dist` and
   `package.json`, `golem-types` symlinked) whose `dist/src/Tool.d.ts` has the one-line
   `Output<B>` fix applied by `sed` (the run fails if the line no longer matches), maps
   `@golemcloud/effect-golem` to it with `compilerOptions.paths`, and prints a `SHIM:` line:
   `node sdks/effect/node_modules/typescript/bin/tsc -p .amp/in/snippets/effect/gen/tsconfig.<f>.json --pretty false`
5. Prints `ALL_OK` only if every style check and all 21 tsc runs passed; otherwise
   `CHECK FAILED` and exit 1.

## Preludes (omitted supporting code)

| Prelude | Provides |
|---|---|
| agents-overview | `classify(body): Effect<string>` (Effect AI `generateText`, OpenAI from `OPENAI_API_KEY`), `applyLabel(inbox, id, label)` (POST) |
| agents-sqlite | `fs` |
| agents-models | `providers` (OpenAI + Anthropic clients from env keys, FetchHttpClient) |
| agents-team | `Facts`, `Plan`, imports of the two generated-client stubs |
| agents-waits | `PaymentEvent`, `Mailer` (MailerAgent spec with `remind`), `createPaymentLink`, `sendSurvey`, `inDays` |
| tools-overview | `Urls`, `summarizeText(page): Effect<string>` (Effect AI, OpenAI); header comment labels the `gen/sdk-fixed` SHIM |
| tools-publish | imports only (the display file imports the CRM client stub itself, as TypeScript does) |
| tools-requests (+ .post) | `Refund`; after the snippet, the refund tool calling the displayed `stripeRefund` |
| tools-permissions | `AgentId`, `Card = Permissions.PersistentCard` |
| tools-custom | `Headings`, `fs`, `BashClient` stub import |
| tools-secrets | `defineConfig` import, `Refund` (`id`, `status`, `customerEmail`), the `refund` tool definition (charge, amount, secret apiKey; body only, no implementation) |
| artifacts-overview | `fs`, `openai` client layer, `Quarter` (`Schema.Struct({ quarter })` with `Schema.isPattern(/^Q[1-4]-\d{4}$/)`), `chartPng(q): Promise<Uint8Array>` (declared), `pageFor(q)`; labelled PRELUDE HELPER `answer(question, toolkit)`: the model tool loop (Effect AI `Chat.fromPrompt` + `chat.generateText({ prompt: [], toolkit })` repeated while `finishReason === "tool-calls"`, at most 5 steps, OpenAI `gpt-4.1`), returning `res.text` and still requiring the toolkit's handlers |
| artifacts-routes | `fs`, `Name` (slug `Schema.isPattern(/^[a-z0-9-]{1,40}$/)`), `Row`, `querySales(region): Effect<Row[]>` (declared) |
| artifacts-files | `SaleRow`, `querySales(team, quarter): Effect<SaleRow[]>`, `drawChart(rows): Uint8Array` (declared) |
| agents-stream | `openai` client layer |
| artifacts-login | imports; `Chart = { askedBy, png }`; `answerWithChart(question): Effect<Uint8Array>` (declared); `userKey(caller)` = JSON of OIDC `[issuer, sub]` (traps on a non-OIDC principal, like TS) |
| agents-readonly | imports only |
| tools-middleware | `Refund` and the same `refund` tool definition as tools-secrets; `askFinance(approval, charge, amount)`, `approved(verdict)` (JSON `approved === true`), `checkRateLimit(principal)` (fails with `MiddlewareError`), `audit(charge, amount, refundId)`; labelled SHIM re-typing `Middleware.typed` (see Differences) |
| artifacts-peruser | imports; `dashboardFor(user, request): Effect<string>` (declared) |
| artifacts-routers | `HttpServerResponse` import; the user's `AnalystAgent` spec (`id: { chat }`, `exportCsv({ id })` returning `WitTypes.AgentStream(WitTypes.Uint8ArraySchema)`, no implementation); `header(name, value): HttpRouter.HttpHeader` (byte value) |
| artifacts-signin | `Invoice`; `invoicesOf(org, issuer, sub)`, `pageFor(question, invoices)` (declared Effects) |

Prelude helpers return `Effect<_, never>` (failures become defects), matching the TypeScript
helpers that throw.

## Libraries

Nothing was invented for Effect; every library is real.

- Golem: `@golemcloud/effect-golem` (`defineAgent`, `method`, `Http.mount/get/post/put` with
  `exposeFiles` (live-file mapping `/*` → `/out/$1`, docs
  `invoke/making-custom-apis/http-routers/semantics.mdx:72`), `auth` (`src/Http.ts:768`), `cors`,
  query bindings `?region={region}` (`src/Http.ts:67`); `Webhook.create`; definition clients `.client.get`, `.schedule`, `ScheduledInvocation.cancel()`;
  `Reflection.getAllAgentTypes` / `getAllToolTypes()`; `Tool.toolDefinition` with
  `requiresFilesystem`, `WitTypes.Secret`, `Secrets.reveal`, `Tool.ok/err`, `.client()`;
  `Permissions.deriveFromWallet/derive/install/revoke`, `Lifespan.fromIso8601`, `Holder.agent`;
  `Principal.PrincipalSchema`; `Unstructured.UnstructuredBinary` + `Unstructured.Option` (an
  optional unstructured result: `none` is a 404); `WitTypes.AgentStream`; `readOnly: true`;
  method `description` (`src/internal/method.ts:104`); generated MCP tool clients' `StartedToolInvocation.collect`;
  typed middleware `Middleware.typed({ name, presented, parameters: Middleware.NoParameters, handler })`
  from `@golemcloud/effect-golem/middleware` (`src/internal/tool/middleware.ts:263-332`; handler context
  `principal`/`underlying` at `:111-122`; callable root `underlying(input)` at `:70-91`;
  `MiddlewareError` with a `constraint-violation` `ToolError` at `:30-36`, exported unchanged at `:925-939`);
  promises `Agents.Promises.create` / `Agents.Promises.await(id)` (`src/Agents.ts:563-596`,
  docs `develop/promises.mdx`, Effect tab); raw routers `HttpRouter.define(name, { mount }).implement(
  Effect.succeed(app))` (`src/HttpRouter.ts:96-171`), the canonical request `HttpRouter.request`
  (`:32`) and byte-valued response headers `HttpRouter.withRawHeaders` (`:43`); router-to-agent calls
  through the agent spec's `client.get` (the router's application runs with `HostServices | Scope`,
  `:64-69`)).
- SQLite: `SqliteClient` from `@golemcloud/effect-golem/sqlite` (Effect SQL over Golem's
  embedded SQLite, per `golem-add-sqlite-effect`).
- Models: Effect AI (`LanguageModel.generateText`, `generateObject`; for tool calling
  `Tool.make` (`effect/src/ai/Tool.ts:1264`), `Toolkit.make` (`ai/Toolkit.ts:574`),
  `toolkit.toLayer` handlers (`ai/Toolkit.ts:102`), the `toolkit` option of `generateText`
  (`ai/LanguageModel.ts:272`), `Chat.fromPrompt` (`ai/Chat.ts:610`) and finish reason
  `"tool-calls"` (`ai/Response.ts:2362`); the Golem Effect SDK's own `src/Ai.ts:3` builds on the
  same `effect/ai` `Tool`/`Toolkit`; all paths under `sdks/effect/node_modules/effect/src/`) with
  `@effect/ai-openai@4.0.0` (`OpenAiLanguageModel.model("gpt-4.1")`) and
  `@effect/ai-anthropic@4.0.0` (`AnthropicLanguageModel.model("claude-sonnet-4-5")`).
- HTTP: `effect/http` (`HttpClientRequest`, `HttpClient.execute`, `HttpClientResponse`,
  `FetchHttpClient.layer`).
- Durable sleep: `Effect.sleep("7 days")`. The Effect SDK has no dedicated sleep API; Effect's
  clock uses the runtime's `setTimeout`, the same WASI clock path that makes the TypeScript
  `node:timers/promises` sleep durable (the SDK's own `Saga` retry delays use `Effect.sleep`).

## Stubs (generated clients)

| Import | Stand-in for | Source of the shape |
|---|---|---|
| `research-agent-guest-client` (`ResearchAgent.get(topic)`, `gather()`, `refresh`) | client generated by `golem build` for a Rust agent listed in `dependencies.agents` | `cli/golem-cli/src/bridge_gen/effect/effect_guest.rs`; `docs/src/content/v1.6/develop/rpc.mdx` (Effect tab) |
| `slack-agent-guest-client` (`SlackAgent.get(channel)`, `post(text)`) | same | same |
| `bash-tool-guest-client` (`BashClient.newClient().run(script, cwd, timeout)`) | Effect client generated for the built-in `bash@0.2.1` tool | `cli/golem-cli/src/bridge_gen/effect/effect_tool.rs`, `typescript/tool.rs` (Effect dialect); `docs/.../golem-generating-tool-clients.mdx` |
| `crm-find-customer-tool-guest-client` (`CrmFindCustomerClient.newClient().crm_find_customer(email)` → `Effect<StartedToolInvocation<{ structured: { name, plan }, content }, McpToolError>, …, ToolRequirements \| Scope>`; also `client`) | Effect client generated for the MCP-imported tool `crm_find_customer` (input `{ email }`, `additionalProperties: false`; output schema `{ name, plan }`) | `typescript/tool.rs` `write_effect_leaf` (a leaf with stdout returns a scoped `StartedToolInvocation`), `effect/effect_tool.rs` (`export const client`), `golem-mcp-import/src/tool.rs` (result `{ structured, content }`, optional stdout, error `mcp-tool-error: string`), `cli/golem-cli/tests/bridge_gen/mcp_import.rs`; `docs/src/content/v1.6/invoke/mcp.mdx` (Effect tab) |

The stubs are `declare class` types built only from real exported SDK types (`Bridge.RemoteAgentHandle`,
`Bridge.RemoteCallError`, `Bridge.ConnectionRequirements`, `BridgeTool.ToolRuntimeError`,
`BridgeTool.ToolRequirements`, `BridgeTool.StartedToolInvocation`). They have no runtime body. Generating the real clients needs a
running Golem environment with the provider components; the stub signatures follow the generator
code exactly (positional constructor arguments, Effect results, `trigger` / `schedule` /
`scheduleCancelable` on stream-free methods).

## Negative controls (`neg.sh`)

Each case copies one display file to `neg/<case>/<chapter>/<feature>/effect/`, applies one `sed`
mutation, runs `check.sh neg/<case>` and requires that snippet's tsc run to fail and `ALL_OK` to
be absent. It then checks there is one case per display file, reruns `check.sh` on the real files
and prints `NEG_OK`.

| Case | Mutation | Error |
|---|---|---|
| triage-decode-wrong-schema | `decodeUnknownEffect(Label)` → `(Email)` | TS2345 |
| sqlite-wrong-option | `SqliteClient.make({ filename` → `{ path` | TS2353 |
| models-old-params | method `input:` → `params:` (old API) | TS2769 |
| team-call-reflection-value | `Reflection.getAllAgentTypes` called as a function | TS2349 |
| waits-positional-id | `Mailer.client.get({ orderId })` → `get(orderId)` | TS2345 |
| custom-misspelt-option | `requiresFilesystem` → `requiresFileSystem` | TS2561 |
| digest-wrong-result | `summarize` `.returns(Schema.String)` → `Schema.Number` | TS2322, TS2345 |
| publish-object-input | `.crm_find_customer(email)` → `({ email })` (generated clients are positional) | TS2345 |
| requests-amount-type | `amount: number` → `bigint` (the refund tool passes `Schema.Int`) | TS2345 |
| permissions-ts-lifespan | `Lifespan.fromIso8601` → `Lifespan.iso8601` (TS SDK name) | TS2339 |
| analyst-ai-sdk-input-schema | `Tool.make` option `parameters:` → `inputSchema:` (Vercel AI SDK name) | TS2353 |
| app-query-typo | `Http.get("/sales?region={region}")` → `{regoin}` (unbound query variable) | TS2769 |
| chart-ts-inline-tag | `{ _tag: "inline", ... }` → `{ tag: "inline", ... }` (TS SDK shape) | TS2322 |
| stream-endpoint-not-array | `http: [Http.post(…)]` → `http: Http.post(…)` (TS SDK shape) | TS2769 |
| login-ts-undefined-none | `return Option.none()` → `return undefined` (TS SDK shape) | TS2322 |
| readonly-bad-flag | `readOnly: true` → `readOnly: "yes"` | TS2769 |
| middleware-ts-create-promise | `Agents.Promises.create` → `Agents.Promises.createPromise` (TS SDK name) | TS2339 |
| peruser-ts-returns | method `success:` → `returns:` (TS SDK name) | TS2769 |
| routers-ts-implement-raw | `Exports.implement(` → `Exports.implementRaw(` (TS SDK name) | TS2551 |
| signin-ts-flat-principal | `const { issuer, sub } = caller.val` → `= caller` (TS SDK shape) | TS2339 |
| secrets-reveal-not-borrow | `config.stripeKey.borrow` → `.get` (a revealed `Redacted<string>`, not a handle) | TS2741 |

`tools/requests` uses no Golem API itself; its control breaks the displayed function's contract
with the Golem refund tool spliced after it.

## Differences from the TypeScript snippets

- Handlers are Effects: method failures become defects with `Effect.orDie` / `Effect.die` /
  `throw` inside `Effect.sync` (all trap, like a TypeScript throw); permission calls keep their
  typed errors instead of `.unwrap()`.
- `agents/overview`: `Label.parse(await classify(...))` becomes `classify(...)` piped into
  `Schema.decodeUnknownEffect(Label)` (two lines); `z.record` is `Schema.Record`.
- `agents/models`: the model is chosen by providing `OpenAiLanguageModel.model(...)` /
  `AnthropicLanguageModel.model(...)`; `Output.object({ schema })` becomes `generateObject`.
- `agents/team`: generated Effect clients resolve with `yield* X.get(...)`, so the awaited call and
  the Slack trigger each take two lines (both lines of the awaited call are highlighted for the
  one TS line); the handler is `Effect.scoped` because the bridge connection requires a `Scope`.
  The reflection `tools` line is split; its first line is highlighted.
- `agents/waits`: the reminder client takes two statements (`Mailer.client.get`, then
  `.remind.schedule`); both are highlighted, like the two TS lines.
- `agents/waits` (sleep): `Effect.sleep("7 days")` replaces `node:timers/promises` (no import line).
- `tools/overview`: `fetch(url).text()` becomes `HttpClient.get(url)` + `res.text` (two lines,
  the first highlighted); `FetchHttpClient.layer` is passed to `implement`. The root command is
  called as `summarize.client()({ url })`. SDK typing bug (same as `tools/custom`, see below):
  the client types the result as `void`, so `out.push(...)` into `string[]` fails with TS2345
  against the real declarations (verified). The snippet keeps the natural code; `check.sh`
  compiles it against `gen/sdk-fixed` (SHIM, labelled in the prelude and in the run output).
- `tools/publish`: the generated Effect client returns an Effect of a `StartedToolInvocation`, so
  the call is `yield*`-ed inside `Effect.scoped`. The SDK has `StartedToolInvocation.collect`
  (`sdks/effect/src/BridgeTool.ts:52`), an Effect rather than a method, so the brief's "Effect has
  no `.collect()`" does not hold and the snippet uses `yield* call.collect`, mirroring TS. The
  rejected-result check is `yield* Effect.fromResult(result)` (fails with the tool error, which
  `Effect.orDie` turns into a trap like the TS `throw`). Method `description` exists in the Effect
  SDK; `structured` is typed by the generated client; no shim beyond the client stand-in.
- `tools/custom`: SDK typing bug, visible in the snippet: the typed tool client
  (`outline.client()`, `Tool.client`) types every call result as `void` because `Output<B>` reads
  the optional `BodyModel.output` (`S | undefined` never extends `Schema.Top`). The snippet
  re-decodes the result with `Schema.decodeUnknownEffect(Headings)`. The root command is invoked
  as `outline.client()({ file })`, not `.outline(...)`.
- `tools/secrets`: the secret is a `Schema.Redacted(Schema.String)` field of a `defineConfig`
  class (`SupportConfig`, shown in the snippet) attached with `config: SupportConfig`; the handler
  yields the config service and passes `yield* config.stripeKey.borrow` (a fresh opaque
  `Capability.Secret` handle, never revealed) as the tool's `apiKey`. The tool is called through
  its typed client `refund.client()({ charge, amount, apiKey })`. Same SDK typing bug as
  `tools/custom`: the client types the result as `void`. Here the snippet shows the natural code
  (`return yield* refund.client()({ ... })`), and `prelude/tools-secrets.ts` holds a labelled
  SHIM that re-types the prelude-defined `refund` client with the tool's declared result. Fix: in `sdks/effect/src/Tool.ts`
  (`type Output<B>`, line 92) test `NonNullable<B["output"]> extends Schema.Top`. Passing a
  config secret to a secret tool parameter is otherwise sound: `.borrow` and
  `WitTypes.Secret(...)` share the `GuestSecretHandle` type and the codec transfers it
  (`test/gol-40-effect-client-capabilities.test.ts` "CLIENT-SECRET" passes).
- `artifacts/overview`: the Vercel AI SDK `tool({ description, inputSchema, execute })` becomes
  Effect AI's split form: the schema half `Tool.make("chart", { description, parameters:
  Quarter, success })` in a `Toolkit` (on display, above the agent), and the handler half
  `Charts.toLayer({ chart: ... })` inside `ask` (highlighted as the counterpart of the TS
  `tool({` line; it closes over `chat`). The tool body is visible and identical in effect
  (`mkdirSync`, the two highlighted writes, the returned page path). The model loop
  (`generateText` + `stopWhen: stepCountIs(5)`) does not fit in 31 lines in Effect AI, whose
  `generateText` resolves one round of tool calls per request; it is the labelled prelude helper
  `answer(question, Charts)`, a real `Chat` loop (see Preludes). The handlers are provided
  visibly with `.pipe(Effect.provide(tools))`; removing that line fails to compile
  (`Handler<"chart">` left in the requirements; verified).
- `artifacts/routes`: `toolDefinition` → `Tool.toolDefinition`, `ok` → `Tool.ok`, the handler is
  `Effect.sync`; `z.array(Row)` → `Schema.Array(Row)`; `init: () => Effect.void`.
- `artifacts/files`: `s.unstructuredBinary({ mimeTypes })` →
  `Unstructured.UnstructuredBinary({ restrictions: [{ mimeType }] })` (two lines); the inline value
  is `{ _tag: "inline", val, mimeType } as const`; `await querySales` → `yield* querySales`.
- `artifacts/login`: `s.principal()` is `Principal.PrincipalSchema`, bound once to `const User`
  (one extra display line) so both method specs fit in 68 columns; OIDC principals are
  `{ tag: "oidc", val: { sub, issuer, ... } }`. `s.option(Png)` is `Unstructured.Option(Png)`;
  `open` returns `Option.none()` (404) or `Option.some({ _tag: "inline", ... })`. To stay within
  31 lines, `ask` is `Effect.map(answerWithChart(question), (png) => charts.push(...))`:
  `Array.prototype.push` returns the new length, which is the chart number the TS returns as
  `this.charts.length` on a separate line; the comment sits directly above `ask:` instead of
  inside the body.

- `agents/overview`, `tools/publish`, `artifacts/overview`, `artifacts/routes`, `artifacts/login`:
  same edits as the TypeScript (comment wording, highlight moved; login's `auth` mount comment is
  gone and a comment sits above the owner check).
- `tools/middleware`: `throw refuse(...)` becomes `yield* refuse(...)`, where `refuse` is
  `Effect.fail(new Middleware.MiddlewareError({ tag: "constraint-violation", val }))` (three lines);
  the handler context is `ctx` (`ctx.principal`, `ctx.underlying(args)`) instead of a destructured
  `{ underlying, principal }`, to fit 68 columns. `createPromise()` / `awaitPromise` are
  `yield* Agents.Promises.create` / `yield* Agents.Promises.await(approval)`; their host errors become
  defects with `Effect.catchTag("AgentsHostError", Effect.die)` (a TS throw), while the policy's own
  rejections stay typed `MiddlewareError`s. Middleware is passed `presented: refund` and
  `parameters: Middleware.NoParameters` (required by the Effect API).
  SDK GAP (SHIM in `prelude/tools-middleware.ts`): `Middleware.typed` infers `R = never` and runs the
  handler with only its optional `layer` (`src/internal/tool/middleware.ts:263-288`, `:848-853`), but
  `Agents.Promises.*` requires the `PromiseClient` host service (`src/Agents.ts:571,580`), whose live
  layer is in `src/host/PromiseClient.ts:31`, blocked by the package exports (`"./host/*": null`).
  Against the real declarations the snippet fails with TS2322 "Type 'PromiseClient' is not
  assignable to type 'never'" (verified); no public layer exists to pass as `layer`. The shim
  re-types `typed` so the handler may require that service (types only). Fix: have
  `invokeToolMiddleware` build `HostLive` into the middleware context (as the agent dispatcher
  does, `src/internal/agent.ts:107`) and widen the handler `R` of `typed`/`universal` to
  `HostServices`, or export a public promise-host layer.
- `artifacts/peruser`: `this.user` is `self.user` on the state object returned by `init`
  (`methods: (self) => ...`); `await dashboardFor(...)` is `yield* dashboardFor(...)`. The mount
  has `auth: true`. Each file is named `crypto.randomUUID()`, the same global as TS (Golem's
  QuickJS runtime: `docs/src/content/v1.6/js-apis.mdx:109`,
  `how-to-guides/common/golem-js-runtime.mdx:22`; typed by `lib.dom` / `@types/node`).
- `artifacts/routers`: `defineHttpRouter(...).mount(...).implementRaw(handler)` is
  `HttpRouter.define(name, { mount: Http.mount("/exports") })` then `Exports.implement(Effect.succeed(app))`
  (two statements, because `implement` returns `void`). The canonical request comes from
  `yield* HttpRouter.request`; the 404 is `HttpServerResponse.empty({ status: 404 })`; the 200 is
  `HttpRouter.withRawHeaders(HttpServerResponse.stream(csv, { status: 200 }), [header(...), ...])`
  with the same byte-valued `header` helper as TS (the body highlight sits on the `stream(csv, ...)`
  line). The agent call takes two lines (`yield* Analyst.client.get(...)`, then
  `yield* analyst.exportCsv(...)`); the first is highlighted.
- `artifacts/signin`: `s.principal()` is `Principal.PrincipalSchema` on its own line (highlighted;
  the `input:` line holds the comment); OIDC fields are `caller.val.issuer` / `caller.val.sub`.

## Known limits (compiles, not proven at runtime)

- Type-check only: no Rollup bundle, no WASM, no deployment. A real component needs
  `golem new --language effect`, the snippet modules imported from `src/main.ts`, the two AI
  providers added to that app, and `golem build` (for `tools/publish`, an MCP import in the
  manifest and a reachable MCP server so the real client can be generated).
- `tools/overview` compiles only with the `gen/sdk-fixed` SHIM until the SDK fix lands.
- `tools/middleware` compiles only with the `Middleware.typed` SHIM; at runtime, today's SDK would
  fail the promise calls (missing `PromiseClient` service) until the middleware host-layer fix lands.
  Middleware attachment to a tool binding is not exercised.
- Runtime-only validations are not exercised: `Http.mount` / endpoint path parsing fallbacks,
  `exposeFiles` mapping compilation (`/*` → `/out/$1`), and agent registration happen at module
  import inside the guest. The Effect AI tool loop in `prelude/artifacts-overview.ts` is not run
  against a model.
- `skipLibCheck: true` (as in the SDK's own tsconfig): errors inside `.d.ts` files are not
  reported; types are still enforced at every use site, as the negative controls show.
- The generated-client stubs are types only; their exact shape could drift from future generator
  output.
- Runtime assumptions not exercised: API keys in the agent environment, model output matching
  `Reply` / `Review`, the model actually calling the `chart` tool, webhooks needing an HTTP API deployment, the bash tool binding with
  filesystem access, OIDC configuration for `auth: true`.

## Re-running

```sh
cd <repo>
(cd sdks/effect && npm ci && npm run build)    # once, or after SDK changes
.amp/in/snippets/effect/check.sh               # expect ALL_OK
.amp/in/snippets/effect/neg.sh                 # expect NEG_OK
```
