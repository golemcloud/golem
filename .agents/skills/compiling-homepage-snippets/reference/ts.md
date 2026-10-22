# TypeScript home-page snippets: how they are compiled and checked

Each home-page feature has its own short snippet, displayed as is:
`website/snippets/<chapter>/<feature>/ts/<display-filename>.ts`. A line ending
in ` //@` is highlighted on the page while its snippet shows;
the page and the harness strip the marker, so it never counts toward any rule.
The harness reads these files in place (never a copy), strips markers,
prepends a per-feature prelude holding the code the snippet omits, and
type-checks the result with the in-tree SDK's strict `tsc`. Every feature,
`tools/publish` included, is a `.ts` code file; there is no YAML snippet any
more (the shared `tools/publish/golem.yaml` was replaced by
`tools/publish/ts/account-agent.ts`).

Re-run (from anywhere):

```shell
<repo>/.amp/in/snippets/ts/check.sh   # ends with ALL_OK
<repo>/.amp/in/snippets/ts/neg.sh     # ends with NEG_OK
```

Both accept an optional snippet root (default
`<repo>/website/snippets`), handy for trying
the rules on a scratch copy.

## Snippets and preludes

| Display file | Prelude |
|---|---|
| `agents/overview/ts/triage-agent.ts` | `prelude/agents-overview.ts` |
| `agents/sqlite/ts/notes-agent.ts` | `prelude/agents-sqlite.ts` |
| `agents/models/ts/draft-agent.ts` | `prelude/agents-models.ts` |
| `agents/team/ts/planner-agent.ts` | `prelude/agents-team.ts` |
| `agents/waits/ts/checkout-agent.ts` | `prelude/agents-waits.ts` |
| `agents/stream/ts/writer-agent.ts` | `prelude/agents-stream.ts` |
| `agents/readonly/ts/stock-agent.ts` | `prelude/agents-readonly.ts` |
| `tools/overview/ts/digest-agent.ts` | `prelude/tools-overview.ts` |
| `tools/custom/ts/build-agent.ts` | `prelude/tools-custom.ts` |
| `tools/requests/ts/stripe-refund.ts` | `prelude/tools-requests.ts` |
| `tools/permissions/ts/orders-access.ts` | `prelude/tools-permissions.ts` |
| `tools/secrets/ts/support-agent.ts` | `prelude/tools-secrets.ts` |
| `tools/middleware/ts/refund-policy.ts` | `prelude/tools-middleware.ts` |
| `tools/publish/ts/account-agent.ts` | `prelude/tools-publish.ts` |
| `artifacts/overview/ts/analyst-agent.ts` | `prelude/artifacts-overview.ts` |
| `artifacts/routes/ts/app-tool.ts` | `prelude/artifacts-routes.ts` |
| `artifacts/peruser/ts/dashboard-agent.ts` | `prelude/artifacts-peruser.ts` |
| `artifacts/files/ts/chart-agent.ts` | `prelude/artifacts-files.ts` |
| `artifacts/routers/ts/export-router.ts` | `prelude/artifacts-routers.ts` |
| `artifacts/signin/ts/billing-agent.ts` | `prelude/artifacts-signin.ts` |
| `artifacts/login/ts/private-chart-agent.ts` | `prelude/artifacts-login.ts` |

The prelude for `<chapter>/<feature>` is always `prelude/<chapter>-<feature>.ts`;
the display filename is free (one `.ts` file per `ts/` directory).

## Prerequisites and versions

- Node.js (checked with v26.0.0) and `python3` (only for `neg.sh`).
- The in-tree TS SDK built once, so that
  `sdks/ts/packages/golem-ts-sdk/dist/index.d.mts` and
  `sdks/ts/packages/golem-ts-sdk/node_modules` exist
  (`cd sdks/ts && npx pnpm install && npx pnpm run build`). The harness uses
  that package's TypeScript 5.9.3 (`node_modules/typescript/bin/tsc`), its
  zod 4.6.7, and its `@types` (`node`, plus the `golem:*` WIT module
  declarations the SDK ships). Rebuild the SDK after SDK source changes, or
  the check runs against stale `dist` types.
- The harness's own `node_modules` (`npm ci` in this directory, from
  `package.json`/`package-lock.json`): `ai` 7.0.148, `@ai-sdk/openai`
  4.0.99, `@ai-sdk/anthropic` 4.0.85, `openai` 6.7.0, `zod` 4.6.7 (the
  harness copy is unused by tsc, which maps `zod` to the SDK's copy).
  `@anthropic-ai/sdk` 1.0.0, `ajv` 8.20.0 and `yaml` 2.9.1 are installed but
  unused since the YAML check was dropped.

## Harness layout

```
<repo>/.amp/in/snippets/ts/
  check.sh        rules + splice + tsc per snippet; ALL_OK
  neg.sh          one planted wrong Golem API use per snippet; NEG_OK
  neg-cases.tsv   <chapter>-<feature> TAB exact text TAB replacement
  prelude/        <chapter>-<feature>.ts, one per snippet (see below)
  stubs/bash-tool-guest-client.ts   stand-in for a generated tool client
  stubs/crm-find-customer-tool-guest-client.ts
                  stand-in for a generated MCP-import tool client
  tsconfig.json   strict, noEmit, ES2022/ESNext, Bundler resolution,
                  skipLibCheck (the SDK dist types report TS2693 with library
                  checking on), noUnusedLocals off; paths (relative, so the
                  directory must stay four levels below the repo root):
                    @golemcloud/golem-ts-sdk -> SDK dist/index.d.mts
                    zod, zod/*               -> SDK node_modules/zod
                    openai, openai/*         -> ./node_modules/openai
                    bash-tool-guest-client   -> ./stubs/bash-tool-guest-client.ts
                    crm-find-customer-tool-guest-client
                      -> ./stubs/crm-find-customer-tool-guest-client.ts
                  typeRoots -> SDK node_modules/@types
                  (ai and @ai-sdk/* resolve from ./node_modules normally)
  package.json, package-lock.json, node_modules/
  gen/            GENERATED each run: spliced sources, per-file tsconfigs,
                  tsc output, <f>.src (path of the display file used)
```

The preludes, stubs, tsconfig and node_modules come from the
`synth-harness` type-check harness (`.amp/in/feature-snippets/synth-harness/`)
that was used while designing these snippets; the four `artifacts-*` preludes
are copied unchanged from it.

## What check.sh does

1. Requires a `ts/` directory for each of the 21 features above, rejects
   unexpected feature directories, and requires exactly one `.ts` file in each.
2. Strips highlight markers (`sed -E 's,[ ]+//@$,,'`) into
   `gen/strip/`, printing `hl=<n>` (marked lines) per file, then applies the
   display rules to the stripped file (all lines count), after blanking string
   and template literals:
   - fail on any line over 68 characters or containing a tab;
   - fail on a `//` comment that does not end with `:`, any block comment,
     and any comment that shares a line with code (`//` or `/*`);
   - fail on a snippet over 31 lines;
   - print `lines=` and `max=` (longest line) for each file.
3. Splices: `gen/<f>.ts` = `prelude/<f>.ts`, a blank line, then the stripped
   display file. Imports a snippet shows itself (for example
   `ai` in `agents/models`, `node:sqlite` in `agents/sqlite`) are legal there
   because ES imports are hoisted; preludes must not import the same names.
   Fails on a prelude with no display file (stale) or a display file without
   a prelude.
4. Compiles each spliced file on its own, printing the command and exit code:
   `node <repo>/sdks/ts/packages/golem-ts-sdk/node_modules/typescript/bin/tsc
   -p gen/tsconfig.<f>.json --pretty false`, where `gen/tsconfig.<f>.json`
   extends `../tsconfig.json` with `files: ["<f>.ts"]`. Errors (first 20
   lines) are shown; full output is in `gen/<f>.out`.
5. Prints `ALL_OK` and exits 0 only if every step passed.

## Preludes (what each snippet omits)

- `agents-overview`: zod, `defineAgent`, `method`; OpenAI client;
  `classify(body)` (asks the model for a label; the snippet parses it with
  its own `Label` enum) and `applyLabel(inbox, id, label)` (fetch helper that
  throws on non-2xx).
- `agents-sqlite`: zod, `defineAgent`, `method`, `node:fs` (the snippet
  imports `DatabaseSync` from `node:sqlite` itself).
- `agents-models`: zod, `defineAgent`, `method` (the snippet imports `ai`,
  `@ai-sdk/anthropic`, `@ai-sdk/openai` itself).
- `agents-team`: `getAllAgentTypes`, `reflection`; `GuestMethod<A, R>`
  modelling the generated client methods (callable plus `trigger`,
  `schedule`, `scheduleCancelable`, as emitted by
  `cli/golem-cli/src/bridge_gen/typescript/mod.rs`); declared
  `ResearchAgent` and `SlackAgent` clients; `Facts`, `Plan` schemas;
  `inDays()` returning a `golem:agent/host` `Datetime`.
- `agents-waits`: `PaymentEvent` schema; a real `MailerAgent` definition
  (`remind`); `inDays()`; `createPaymentLink`, `sendSurvey` fetch helpers.
- `tools-overview`: zod, `defineAgent`, `method`, `ok`, `toolDefinition`;
  OpenAI client; `Urls` (`z.array(z.string())`); `summarizeText(page)`.
- `tools-custom`: `ok`, `toolDefinition`, `node:fs`, `BashClient` from the
  stub; `Headings` schema.
- `tools-requests`: the `refund` tool definition plus `stringType` and
  `stringValue` helpers (kept from the earlier design; the displayed
  `stripeRefund` does not use them, only `fetch`).
- `tools-publish`: zod, `defineAgent`, `method` (the snippet imports
  `CrmFindCustomerClient` from the stub itself).
- `tools-secrets`: the `refund` tool definition and `Refund` schema. It also
  holds two labelled type-only declarations. One brands the raw WIT `Secret`
  (an empty class, so any value would type-check). The other, a SHIM for an
  SDK bug, gives the SDK's config `Secret<T>` the same brand so passing
  `this.config.stripeKey` to the tool client compiles. The bug: a tool
  client should accept a config `Secret<T>` for an `s.secret(T)` parameter,
  but the SDK types that parameter as the raw resource and encodes the
  wrapper as if it were one (`src/schema/markers.ts:742-752`,
  `src/internal/schema-model/wit.ts:910-921`), and the only unwrapping helper,
  `withConfigSecretHandle` (`src/secret.ts:67`), is internal and disposes the
  handle (`src/config.ts:260-272`). The displayed code is the intended API.
- `tools-permissions`: `permissions`; `AgentId` (parameter type of
  `permissions.Holder.agent`); `Card` union of permission card types.
- `artifacts-overview`: zod, `defineAgent`, `http`, `method`, `node:fs`;
  Vercel AI SDK `generateText`, `stepCountIs`, `tool` and an OpenAI model;
  the `Quarter` input schema (regex-validated), `chartPng` and `pageFor`.
- `artifacts-routes`: zod, `defineAgent`, `http`, `method`, `ok`,
  `toolDefinition`, `node:fs`; the `Name` slug schema, `Row`, `querySales`.
- `artifacts-files`: zod, `defineAgent`, `http`, `method`, `s`;
  `querySales(team, quarter)` and `drawChart`.
- `tools-middleware`: the `refund` tool (same shape as `tools-secrets`, plus
  `customerEmail` in `Refund`), `ToolInvokeError`, `createPromise`/`awaitPromise`;
  application helpers `askFinance`, `approved`, `checkRateLimit(principal)`, `audit`.
- `artifacts-peruser`: zod, `defineAgent`, `http`, `method`, `node:fs`; `dashboardFor`.
- `artifacts-routers`: `defineHttpRouter`, `AgentStream`, `s`; the `AnalystAgent`
  whose `exportCsv({ id })` streams an export as CSV; `header` and `empty` helpers.
- `artifacts-signin`: zod, `defineAgent`, `http`, `method`, `s`; `invoicesOf`, `pageFor`.
- `artifacts-login`: zod, `defineAgent`, `http`, `method`, `s`, `Principal`;
  the `Chart` type, `answerWithChart`, and `userKey` (OIDC issuer + sub).
- `agents-stream`: `AgentStream`, `s`; OpenAI client.
- `agents-readonly`: zod and SDK imports only.

## Invented libraries and stubs

- Invented libraries: none. Every third-party library is real and installed:
  `ai` + `@ai-sdk/openai` + `@ai-sdk/anthropic` (agents/models), `openai`
  (agents/overview, agents/stream, tools/overview, artifacts/overview),
  `node:sqlite` and `node:fs` typed by
  the SDK's `@types/node`.
- `stubs/bash-tool-guest-client.ts` (import path `bash-tool-guest-client`):
  declared `BashClient.newClient().run(script, cwd, timeout)` returning
  `{ exitCode, stdout, stderr }`, standing in for the client `golem build`
  generates for the built-in `bash@0.2.1` release, shaped after
  `docs/src/content/v1.6/how-to-guides/common/golem-generating-tool-clients.mdx`. Needed because the real one
  only exists inside a built application.
- `stubs/crm-find-customer-tool-guest-client.ts` (import path
  `crm-find-customer-tool-guest-client`), labelled in the file as a stand-in:
  declared `CrmFindCustomerClient.newClient().crm_find_customer(email)`
  returning the SDK's real `bridge.StartedToolInvocation<R>` with
  `R = { structured: { name: string; plan: string }; content: unknown }`.
  It models the client `golem build` generates for an MCP import `crm` whose
  tool `find_customer` takes `{ email: string }` (`additionalProperties:
  false`) and declares an output schema with string `name` and `plan`. Shape
  after `cli/golem-cli/tests/bridge_gen/mcp_import.rs` (TypeScript cases:
  positional fields, typed `result.structured`) and the TypeScript example in
  `docs/src/content/v1.6/invoke/mcp.mdx` ("Call the imported tool":
  `newClient().<prefix>_<tool>(...)`, `await invocation.collect()`,
  `result.status === "rejected"`). `.collect()` and the settled-result type
  are the SDK's own, so only the method name and `R` come from the stub.
- `ResearchAgent`, `SlackAgent` (in `prelude/agents-team.ts`): `declare const`
  shapes of generated cross-component agent clients, for the same reason.

## Negative controls (neg.sh)

`neg.sh` first requires `check.sh` to pass, then for each snippet takes its one
row in `neg-cases.tsv`, requires the text to occur exactly once in the display
file, replaces it in memory, writes prelude + mutant to `gen/neg-<f>.ts`
(the display file is never modified) and requires `tsc` to fail.

| Snippet | Planted change | Error |
|---|---|---|
| agents-models | `write({ idea })` → `write({ topic })` | TS2339 |
| agents-overview | `id.inbox` → `id.mailbox` in `init` | TS2339 |
| agents-sqlite | `add({ text })` → `add({ body })` | TS2339 |
| agents-team | `reflection.getAllToolTypes()` → `getAllTools()` | TS2339 |
| agents-waits | `reminder.cancel()` → `reminder.abort()` | TS2339 |
| agents-stream | `AgentStream.from(` → `AgentStream.of(` | TS2339 |
| agents-readonly | `readOnly: true` → `readOnly: 'yes'` | TS2769 |
| tools-overview | `summarize({ url })` → `summarize({ link: url })` | TS2353 |
| tools-custom | `outline({ file })` → `outline({ path: file })` | TS2353 |
| tools-requests | `method: 'POST',` + `idempotencyKey: charge,` | TS2769 |
| tools-permissions | `Lifespan.iso8601('PT1H')` → `'PT1H'` | TS2345 |
| tools-secrets | `this.config.stripeKey` → `.stripeKey.get()` | TS2322 |
| tools-publish | `result.status === 'rejected'` → `=== 'error'` | TS2367 |
| artifacts-overview | exposeFiles `path:` → `file:` | TS2769 |
| artifacts-routes | tool `html` positional `z.string()` → `z.number()` | TS2345 |
| artifacts-files | inline return `tag: 'inline'` → `'raw'` | TS2322 |
| artifacts-login | `made?.askedBy` → `made?.asked` | TS2551 |
| tools-middleware | `checkRateLimit(principal)` → `(args.charge)` | TS2345 |
| artifacts-peruser | `id.user` → `id.users` | TS2551 |
| artifacts-routers | `exportCsv({ id })` → `({ n })` | TS2353 |
| artifacts-signin | `{ issuer, sub }` → `{ issuer, subject }` | TS2339 |

`tools/requests` contains no Golem API call (Golem adds the idempotency key
itself), so its control plants the tempting wrong usage of passing the key
yourself, which `fetch`'s `RequestInit` rejects. When a snippet changes, update
its row so the text still occurs exactly once.

## Known limits

- Only type-checking. Nothing is bundled, built into a component or run.
- The raw host `Secret` type is structurally empty, so tsc accepts any object
  where a secret handle belongs; secret forwarding is not proven.
- Generated clients (`BashClient`, `CrmFindCustomerClient`, `ResearchAgent`,
  `SlackAgent`) are stubs, so calls on them are checked only against the stub
  shapes. MCP import, the tool's JSON schemas and exposing `describe` as an
  MCP tool are not exercised.
- Not proven at runtime: webhook delivery, durable sleep and schedule
  behaviour, permission-card lifetimes, idempotency, gateway serving of
  exposed and live files, the board page's relative POST, principal
  checks, read-only caching, and whether `node:sqlite`, `openai` and the
  `ai` SDK work inside the component.
- The 21-feature list in `check.sh` is hard-coded; add a feature there and a
  prelude plus a `neg-cases.tsv` row when the page gains one.

## artifacts/overview: UUID file names (Oct 22 2026)

The chart tool names the PNG and the HTML page with one fresh UUID, as artifacts/peruser does, and `pageFor(q, name)` points the page's `<img>` at `<name>.png`. `const name = crypto.randomUUID()`. To stay within 31 lines, the `methods` block of the agent definition is folded onto two lines, as in the Effect snippet. No comment explains the UUID (owner ruling). Re-verified: `check.sh` ALL_OK, `neg.sh` passing.
