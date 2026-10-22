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
| agents/overview | `website/snippets/agents/overview/effect/support-agent.ts` |
| agents/language | `website/snippets/agents/language/effect/cart-agent.ts` |
| agents/sqlite | `website/snippets/agents/sqlite/effect/notes-agent.ts` |
| agents/models | `website/snippets/agents/models/effect/draft-agent.ts` |
| agents/team | `website/snippets/agents/team/effect/planner-agent.ts` |
| agents/waits | `website/snippets/agents/waits/effect/checkout-agent.ts` |
| tools/overview | `website/snippets/tools/overview/effect/refund-tool.ts` |
| tools/custom | `website/snippets/tools/custom/effect/build-agent.ts` |
| tools/requests | `website/snippets/tools/requests/effect/stripe-refund.ts` |
| tools/permissions | `website/snippets/tools/permissions/effect/orders-access.ts` |
| tools/secrets | `website/snippets/tools/secrets/effect/support-agent.ts` |
| artifacts/overview | `website/snippets/artifacts/overview/effect/site-agent.ts` |
| artifacts/routes | `website/snippets/artifacts/routes/effect/report-agent.ts` |
| artifacts/routers | `website/snippets/artifacts/routers/effect/exports-router.ts` |
| artifacts/stream | `website/snippets/artifacts/stream/effect/writer-agent.ts` |
| artifacts/login | `website/snippets/artifacts/login/effect/notebook-agent.ts` |
| artifacts/readonly | `website/snippets/artifacts/readonly/effect/page-agent.ts` |

The shared `tools/publish/golem.yaml` is not ported. `tools/secrets` is a per-language snippet
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
stubs/bash-tool-guest-client.ts  stand-ins for `golem build` generated clients
vendor/                  npm project holding @effect/ai-openai@4.0.0 and @effect/ai-anthropic@4.0.0
node_modules/            symlinks created by check.sh:
  effect                    -> sdks/effect/node_modules/effect
  @golemcloud/effect-golem  -> sdks/effect
  @types/node               -> sdks/effect/node_modules/@types/node
  @effect/ai-openai         -> vendor/node_modules/@effect/ai-openai
  @effect/ai-anthropic      -> vendor/node_modules/@effect/ai-anthropic
gen/                     generated sources, tsconfigs and tsc output (rebuilt on every run)
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
   and runs, printing the command and exit code:
   `node sdks/effect/node_modules/typescript/bin/tsc -p .amp/in/snippets/effect/gen/tsconfig.<f>.json --pretty false`
5. Prints `ALL_OK` only if every style check and all 17 tsc runs passed; otherwise
   `CHECK FAILED` and exit 1.

## Preludes (omitted supporting code)

| Prelude | Provides |
|---|---|
| agents-overview | `openai` (OpenAiClient.layerConfig from `OPENAI_API_KEY` + FetchHttpClient), `Message = Prompt.MessageEncoded`, `Reply`, `Approval`, `Ticket`, `notifyReviewer`, `refundOrder` |
| agents-language | `placeOrder` (POST, decode `{ id }`) |
| agents-sqlite | `fs` |
| agents-models | `providers` (OpenAI + Anthropic clients from env keys, FetchHttpClient) |
| agents-team | `Facts`, `Plan`, `inDays`, imports of the two generated-client stubs |
| agents-waits | `PaymentEvent`, `Mailer` (MailerAgent spec with `remind`), `createPaymentLink`, `sendSurvey`, `inDays` |
| tools-overview | `Refund`, `stripeRefund` (same body as the requests snippet) |
| tools-requests (+ .post) | `Refund`; after the snippet, the refund tool calling the displayed `stripeRefund` |
| tools-permissions | `AgentId`, `Card = Permissions.PersistentCard` |
| tools-custom | `Headings`, `fs`, `BashClient` stub import |
| tools-secrets | `defineConfig` import, `Refund`, the `refund` tool definition from tools/overview (body only, no implementation) |
| artifacts-overview | `fs`, `streamPage(brief): Stream<string>` |
| artifacts-routes | `fs`, `frontEnd(team): string`, `render(month): Effect<string>` |
| artifacts-routers | `HttpServerResponse` import; `Export`, an unimplemented `defineAgent` spec for `ExportAgent` (id `{ id: Schema.String }`, method `csv` with `success: WitTypes.AgentStream(WitTypes.Uint8ArraySchema)`) used only through its typed client |
| artifacts-stream | `openai` client layer |
| artifacts-login, artifacts-readonly | imports only |

Prelude helpers return `Effect<_, never>` (failures become defects), matching the TypeScript
helpers that throw.

## Libraries

Nothing was invented for Effect; every library is real.

- Golem: `@golemcloud/effect-golem` (`defineAgent`, `method`, `Http.mount/get/post/put` with
  `exposeFiles`, `auth`, `cors`; `HttpRouter.define(name, { mount }).implement(Effect.succeed(application))` with
  `HttpRouter.request` (the original canonical request) and Effect `HttpServerResponse` responses;
  `Webhook.create`; definition clients `.client.get`, `.schedule`, `ScheduledInvocation.cancel()`;
  `Reflection.getAllAgentTypes` / `getAllToolTypes()`; `Tool.toolDefinition` with
  `requiresFilesystem`, `WitTypes.Secret`, `Secrets.reveal`, `Tool.ok/err`, `.client()`;
  `Permissions.deriveFromWallet/derive/install/revoke`, `Lifespan.fromIso8601`, `Holder.agent`;
  `Principal.PrincipalSchema`; `WitTypes.AgentStream`; `readOnly: true`).
- SQLite: `SqliteClient` from `@golemcloud/effect-golem/sqlite` (Effect SQL over Golem's
  embedded SQLite, per `golem-add-sqlite-effect`).
- Models: Effect AI (`LanguageModel.generateText`, `generateObject` from `effect/ai`) with
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

The stubs are `declare class` types built only from real exported SDK types (`Bridge.RemoteAgentHandle`,
`Bridge.RemoteCallError`, `Bridge.ConnectionRequirements`, `BridgeTool.ToolRuntimeError`,
`BridgeTool.ToolRequirements`). They have no runtime body. Generating the real clients needs a
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
| overview-ts-webhook | `Webhook.create` → `Webhook.createWebhook` (TS SDK name) | TS2339 |
| language-wrong-success | `add` `success: Schema.Int` → `Schema.String` | TS2322 |
| sqlite-wrong-option | `SqliteClient.make({ filename` → `{ path` | TS2353 |
| models-old-params | method `input:` → `params:` (old API) | TS2769 |
| team-call-reflection-value | `Reflection.getAllAgentTypes` called as a function | TS2349 |
| waits-positional-id | `Mailer.client.get({ orderId })` → `get(orderId)` | TS2345 |
| custom-misspelt-option | `requiresFilesystem` → `requiresFileSystem` | TS2561 |
| overview-undeclared-error | `Tool.err("http"` → `Tool.err("timeout"` | TS2322 |
| requests-amount-type | `amount: number` → `bigint` (the refund tool passes `Schema.Int`) | TS2345 |
| permissions-ts-lifespan | `Lifespan.fromIso8601` → `Lifespan.iso8601` (TS SDK name) | TS2339 |
| site-stream-item-type | `AgentStream(Schema.String)` → `AgentStream(Schema.Number)` | TS2322 |
| routes-file-not-path | `exposeFiles` entry `path:` → `file:` | TS2353 |
| routers-ts-implement-raw | router `.implement(` → `.implementRaw(` (TS SDK name) | TS2551 |
| stream-endpoint-not-array | `http: [Http.post(…)]` → `http: Http.post(…)` (TS SDK shape) | TS2769 |
| login-ts-principal-shape | `caller.val.sub` → `caller.sub` (TS SDK shape) | TS2339 |
| readonly-bad-flag | `readOnly: true` → `readOnly: "yes"` | TS2769 |
| secrets-reveal-not-borrow | `config.stripeKey.borrow` → `.get` (a revealed `Redacted<string>`, not a handle) | TS2741 |

`tools/requests` uses no Golem API itself; its control breaks the displayed function's contract
with the Golem refund tool spliced after it.

## Differences from the TypeScript snippets

- Handlers are Effects: method failures become defects with `Effect.orDie` / `Effect.die` /
  `throw` inside `Effect.sync` (all trap, like a TypeScript throw); permission calls keep their
  typed errors instead of `.unwrap()`.
- `agents/overview`: `LanguageModel.generateObject({ prompt: history, schema: Reply })` replaces
  `chat.completions.parse` + `zodResponseFormat` + `Reply.parse`; the schema decode happens inside
  `generateObject`. The OpenAI client layer (`openai`) is provided at the end of the handler.
- `agents/models`: the model is chosen by providing `OpenAiLanguageModel.model(...)` /
  `AnthropicLanguageModel.model(...)`; `Output.object({ schema })` becomes `generateObject`.
- `agents/team`: generated Effect clients resolve with `yield* X.get(...)`, so the Slack trigger
  takes two lines; `research.refresh.schedule(inDays(1))` needs a `Scope` (`Effect.scoped`).
- `agents/waits`: `Effect.sleep("7 days")` replaces `node:timers/promises` (no import line).
- `tools/overview`: Effect tool errors carry a payload schema, so `http` carries the status
  (`Tool.err("http", res.status)`); the error kind is `"runtime-error"`.
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
- Comment placement differs where the Effect structure differs: `artifacts/overview` builds the
  `save` effect before the stream so the completion comment fits in 68 columns.
- `artifacts/routes`: init receives the id fields directly (`({ team })`) and writes the front-end in
  `Effect.sync`; the state is `void`, so `methods: () => ...`.
- `artifacts/routers`: the Effect SDK has no `implementRaw`; a router is implemented with an Effect
  application (`HttpRouter.define(...).implement(Effect.succeed(app))`). The handler reads the
  original full path and method from `yield* HttpRouter.request` (not the mount-relative Effect
  `HttpServerRequest`), so it matches `/exports/<id>.csv` like the TypeScript raw handler. A miss
  returns `HttpServerResponse.empty({ status: 404 })`. The typed client resolves in two steps
  (`yield* Export.client.get({ id })`, then `yield* exp.csv({})`; Effect RPC input is always one
  object, so `csv()` without `{}` is a type error); both lines are highlighted for the one TS line.
  The received `Stream<Uint8Array, unknown>` is forwarded directly with
  `HttpServerResponse.stream(rows, { contentType, headers })`; that line is the counterpart of
  `body: rows`. No shim was needed.
- `artifacts/login`: OIDC principals are `{ tag: "oidc", val: { sub, ... } }`.

## Known limits (compiles, not proven at runtime)

- Type-check only: no Rollup bundle, no WASM, no deployment. A real component needs
  `golem new --language effect`, the snippet modules imported from `src/main.ts`, the two AI
  providers added to that app, and `golem build`.
- Runtime-only validations are not exercised: `Http.mount` / endpoint path parsing fallbacks,
  `exposeFiles` mapping compilation, forwarding an RPC byte stream into a router response body, and agent registration happen at module
  import inside the guest.
- `skipLibCheck: true` (as in the SDK's own tsconfig): errors inside `.d.ts` files are not
  reported; types are still enforced at every use site, as the negative controls show.
- The generated-client stubs are types only; their exact shape could drift from future generator
  output.
- Runtime assumptions not exercised: API keys in the agent environment, model output matching
  `Reply` / `Review`, webhooks needing an HTTP API deployment, the bash tool binding with
  filesystem access, OIDC configuration for `auth: true`.

## Re-running

```sh
cd <repo>
(cd sdks/effect && npm ci && npm run build)    # once, or after SDK changes
.amp/in/snippets/effect/check.sh               # expect ALL_OK
.amp/in/snippets/effect/neg.sh                 # expect NEG_OK
```
