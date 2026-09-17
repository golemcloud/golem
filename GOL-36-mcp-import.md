# GOL-36: MCP import — work-in-progress specification and plan

Status: implementation in progress. Steps 1–7 and 9 completed after tests, Oracle
review and bounded bug-finder loops. Steps 8 and 10 remain open: the middleware
dependency is not integration-ready, and final combined acceptance must include it.
The resource-budget boundary has provisional user approval and must be revisited
in the final review.
Middleware remains an implementation dependency. The finalized planning snapshot
is attached to GOL-36 in Linear.
The user approved a fixed one-day limit for resolver operation timeouts and
refresh intervals. The correction passed 28 targeted tests and Oracle review;
periodic-refresh bug-finder run 5 resolved the timeout finding with no new
findings or active checkpoint. Public metadata inspection, refresh, generated
clients, deployment warnings and operator documentation are now implemented and
verified. Combined OAuth/agent acceptance, quota enforcement, public-oplog
rendering, protocol validation and generated config checks pass; middleware
integration is still blocked on its dependency transfer.

This is the living record of the requirements, decisions, implementation plan, and
open details discussed in the planning thread. Update this file in place as the
discussion progresses; do not leave consequential decisions only in chat.

## Sources and authority

- [GOL-36: MCP import](https://linear.app/golem-cloud/issue/GOL-36/mcp-import)
- [Agent Tools specification, part 1: calling-agent context (§4.5–4.6)](https://linear.app/golem-cloud/document/agent-tools-imported-specification-part-1-of-3-219308fd7182)
- [Agent Tools specification, part 2: imports and projection (§5.7.2–5.7.3)](https://linear.app/golem-cloud/document/agent-tools-imported-specification-part-2-of-3-5d7971fdd24a)
- [Agent Tools specification, part 3: middleware](https://linear.app/golem-cloud/document/agent-tools-imported-specification-part-3-of-3-7a6ce17c8422)
- [Planning thread](https://ampcode.com/threads/T-01a0a44d-b3d8-706a-9f0e-bece137e98a9)

Follow the original Agent Tools specification, with the explicit user
clarifications below. Do not turn implementation gaps into product scope
reductions. In particular, OAuth and middleware remain in scope. Follow current
repository contracts where the historical specification describes machinery that
has since changed. Resolve an actual conflict explicitly rather than silently
choosing a different behavior. No backward-compatibility work is planned.

## Settled requirements

### Imports are ordinary tools to agents

External Streamable HTTP MCP servers become first-class environment tool sources.
Agents enumerate and invoke imported tools through the existing discovery and
`tool-rpc` interfaces. Projected metadata also feeds typed-client generation.

Each upstream tool projects to one top-level Golem tool with a root command body.
The import declaration supplies the binding for agents in the environment:

- No top-level `tools.<name>` declaration is required for an import.
- No explicit environment tool binding is required to make it available.
- Environment and per-tool middleware apply as specified for other tools.
- Import bindings do not use the native tool binding's `version` or `parameters`
  fields; credential/context narrowing still follows the specification's rules.

The bridge runs host-side, reusing native-tool execution. It is not a deployable
component or a new agent. A stable synthesized `implemented-by` component ID is a
discovery identity only.

Per-name middleware belongs in `environments.<env>.tools.<projected-name>`, as
specified in §5.7.2; do not invent an import-level replacement. Validate its static
configuration at deploy time. A syntactically valid name that might come from an
import but is not currently discoverable produces a warning, not a deployment
failure. Validate compatibility on demand/refresh once metadata is available, and
keep the binding so a later upstream addition can satisfy it. Invalid middleware
configuration still fails normal static validation. Universal chains always apply.

### Manifest and projection

`mcp.imports.<environment>` is an ordered list of entries:

| Field | Contract |
| --- | --- |
| `url` | Required Streamable HTTP endpoint. |
| `auth` | Configured bearer/basic authentication; mutually exclusive with `securityScheme`. |
| `securityScheme` | Reference to a manifest-defined security scheme, including OAuth. |
| `prefix` | Optional kebab-case prefix, joined with `-`. |
| `include` / `exclude` | Mutually exclusive glob filters on sanitized upstream names, before prefixing. |
| `version` | Optional MCP protocol version override, not a tool release version. |

Sanitize upstream names by lowercasing, replacing underscores and other
disallowed characters with hyphens, collapsing runs, and stripping leading and
trailing hyphens. Preserve the original upstream name for `tools/call`.

Precedence is **native tools > earlier import > later import**. This includes
application component tools. Drop lower-precedence collisions with warnings;
do not convert the import collision rule into native deployment collision errors.
Provide the specified startup warnings and useful demand/refresh diagnostics.

Follow §5.7.3 for schemas, documentation, annotations, results, and errors:

- Preserve the four standard annotations: read-only, destructive, idempotent,
  and open-world. Hints are not authorization or deduplication guarantees.
- Preserve exact upstream JSON property names through projection/conversion.
- Use stdout for simple text/binary output and proper structured values for mixed
  content and structured results, as clarified below.
- `isError: true` becomes synthetic `mcp-tool-error`: runtime-error, exit code 1,
  string payload, surfaced through the normal custom-error path.
- Map protocol errors to invalid-input or custom-error as specified. A removed
  tool maps to invalid-tool-name; input drift maps to invalid-input.
- Missing or invalid declared successful output maps to `invalid-result`
  (`SerializableToolError::InvalidResult`), not invalid-input or an invented value.
- Do not claim lossless Golem → MCP → Golem round-tripping.

Agreed projection-failure policy:

- Implement the specified mappings without silently weakening validation or
  discarding fields. First try to represent each construct faithfully using
  Golem's schema facilities; this policy is not an arbitrary schema-subset limit.
- If a tool's definition is invalid or cannot be projected faithfully, exclude
  that tool, not the entire import. Keep other valid tools available.
- Report the excluded tool and precise reason during discovery/refresh, including
  manual refresh output.
- If a previously available tool becomes unrepresentable after a successful
  fetch, stop advertising its old definition. Historical calls still replay from
  their recorded snapshots.

### Simple output uses stdout; mixed output uses structured values

User clarification: generated typed clients are the primary consumers of tools,
so mixed content needs proper structured output rather than an opaque byte stream.
Tools that just return a string, binary data, or a similar simple payload should
still use stdout. Do not turn typed-client priority into a ban on stdout.

- Preserve stdout for simple text/binary output; do not require these payloads to
  be wrapped in a generic content record solely for uniformity.
- Represent mixed content structurally as an ordered list of typed variants for
  text, image, audio, resource links, and embedded resources. Preserve associated
  MIME information, payloads, and relevant metadata using existing schema
  facilities, including the multimodal list-of-variants convention where useful.
  Do not serialize mixed content as JSON on stdout.
- Preserve the projected upstream output type when `outputSchema` is declared;
  do not erase it into an opaque JSON string or byte buffer. Define the boundary
  with simple-payload stdout projection without silently violating a declared
  output schema.
- Preserve both upstream structured values and accompanying content when present,
  using the stable output contract below.
- Advertise a stable result/stdout contract at discovery time. MCP `outputSchema`
  describes `structuredContent`, not the separate content block list, so it does
  not generally identify tools that return only simple content. Declare both
  output possibilities once, using the contract below, rather than changing a
  generated client's schema after observing an individual response.
- Resource links remain links; automatic fetching is not part of projection.

The original §5.7.3.7 example's simple-output stdout behavior is retained; its
`result: none` metadata shape is superseded by the fixed result record below.
The user's clarification supplies the mixed-content structured-output requirement;
the earlier proposed blanket replacement of stdout is withdrawn. Existing
native-tool stdout and the MCP export contract are unchanged.

#### Stable output contract

Use one discovery-time schema per projected tool, with optional stdout and a
structured result whose content arm distinguishes no content, simple streamed
content, and structured blocks. This implements the discussed fixed-schema
approach; selecting a variant at runtime does not change the client type.
The exact record/variant layout below is an engineering proposal implementing
the agreed behavior, not a separately mandated user API.

Conceptual shape (actual identifiers follow existing schema/codegen conventions):

```text
result {
  structured: T,                         // when outputSchema declares T
  content: none
         | streamed { mime-type, annotations }
         | blocks(list<ContentBlock>)
}
stdout: optional bytes
```

- With `outputSchema`, `structured` uses the projected type `T`; successful
  responses must supply valid `structuredContent`. Invalid/missing declared
  results fail validation, rather than yielding an invented default.
- Without `outputSchema`, use an optional field for otherwise untyped
  `structuredContent`, following the existing `serde_json::Value` schema mapping
  (a JSON string). This narrow fallback preserves undeclared JSON; it never
  replaces a declared type or the typed content-block variants.
- Empty `content` selects `none`. A single text block writes UTF-8 to stdout; a
  single image/audio block writes decoded bytes. These select `streamed`, retaining
  MIME type and annotations in its descriptor. Empty text still selects streamed.
  Text uses `text/plain; charset=utf-8`; image/audio use their declared MIME types.
- Multiple content blocks select `blocks`, even when their types match: preserve
  boundaries and order rather than inventing separators. Resource links and
  embedded resources also use blocks, including a single resource, so identity
  and metadata are not lost. Preserve supported extension metadata using existing
  JSON value conventions rather than silently dropping it.
- Simple content is not duplicated in the structured block list. A response with
  a declared structured value and a simple accompanying content block can return
  both its typed value and stdout. Do not guess that a text block duplicates the
  structured value and remove it.
- Every projected root body declares stdout with `mime: ["*/*"]`,
  `required: false`. If a raw caller omits it, simple output bytes are not delivered;
  the result still selects `streamed`. Do not reroute bytes into another arm.
  Generated clients must expose the ordinary stdout attachment/consumption path
  and exercise it in tests; do not add an MCP-only stream API or change other tools.
  Always settle attached stdout, including calls selecting `none` or `blocks`.
- `isError: true` bypasses successful-output validation and returns the specified
  synthetic error, not a successful result record or stdout. A single text block
  supplies its text unchanged; otherwise use compact JSON serialization of the
  ordered content array as the string payload (including `[]` for no blocks).
  Upstream `structuredContent` on an error is not returned as a successful typed
  value. Error rendering is separate from the success-output projection rules.
- Record the complete remote response before publishing stdout or the result, as
  specified below. Reconstruct the selected arm, value, descriptor, and bytes
  deterministically from that response and the admission-time projection.
  Test generated-client decoding and stream settlement, not only JSON snapshots.

### Tool selection follows the running agent

User clarification: registry lookup must use the deployment belonging to the
currently running agent, like its other metadata, and MCP imports must work
exactly like existing tools.

Use the same owner deployment context for native tools, bindings, middleware,
and MCP import declarations. Do not independently select a newer deployment for
an MCP call. Upstream tool metadata remains dynamic within those declarations.

Concretely, preserve the existing live tool lookup: resolve the latest deployment
containing the running owner's `(environment, component ID, component revision)`.
Read the selected revision, native tools, bindings, and import declarations from
one coherent deployment snapshot. This is not an environment-current lookup and
not a permanent worker-to-deployment pin. A config-only redeploy can affect the
next live operation after normal cache invalidation/expiry, just as for native
tools today; replay of an earlier observation must still see its original view.

Record that observation's selected deployment revision as a compact reference,
alongside its dynamic metadata. Rehydrate fixed metadata by exact revision during
replay. This records identity rather than copying full native tool definitions;
it does not change worker creation, update, fork, or revert semantics. A recorded
absence of a deployment stays absent during replay. Existing latest-containing
selection after an environment rollback is unchanged by this task.

Do not introduce new MCP-specific creation, update, or rollback semantics. Earlier
proposals for new deployment-targeting rules and CLI choices are not accepted
requirements. Investigate any discrepancy in the existing native lookup as a
concrete implementation issue, preserving the common lifecycle contract.

### Registry caching is only an optimization

User clarification: periodic refresh and registry caching are optional
optimizations. On a live listing, lookup, or invocation cache miss, fetch the
relevant upstream metadata on demand. Prior listing or deployment-time discovery
must not be required for invocation correctness.

- Distinguish a cache miss, a successful empty list, and a failed fetch.
- Never return tool-not-found solely because the cache is empty.
- Use the same resolution rules for demand fetch and refresh.
- Refresh cannot be blocked by an in-flight invocation.
- Cache loss must not change which source wins a name; it may require an upstream
  fetch and therefore produce a transient resolution failure when upstream is down.
- Scope cached views to the configured import and effective authorization;
  do not leak a credential-specific tool list across callers.
- Deploy-time discovery warms the cache; it does not make upstream metadata
  immutable or eliminate on-demand resolution.

Agreed refresh-failure policy:

- A successful refresh replaces the cached metadata, including reported removals
  and schema changes. Do not retain an old definition merely because its
  replacement is incompatible.
- A failed background refresh retains the last successfully fetched metadata,
  which remains usable, and reports the failure. An upstream invocation can still
  reject input based on stale metadata; handle that failure as the spec requires.
- With no usable cached metadata, listing or invocation fetches on demand.
  Failure follows normal error/retry handling, never an empty-list substitute.
- Manual refresh explicitly reports failure even when old metadata remains usable.
- If successfully refreshed metadata is incompatible with configured middleware,
  reject affected new calls rather than bypassing middleware. Previously recorded
  calls replay from their snapshots.

Exact refresh cadence, cache lifetime, concurrency, and size limits remain runtime
implementation details. Background refresh must use an authorized credential
context, not impersonate an arbitrary agent. Stale metadata is not a fallback for
changed configuration, a different credential context, or revoked authorization.

Resolution publishes a complete paginated upstream observation, never a partially
fetched list. Coalesce concurrent fetches for the same cache key. Before accepting
a lower-precedence import, resolve earlier potentially colliding imports or use
their usable cached observations; an unavailable earlier import with no usable
cache is not permission to promote a later import. A native-name winner does not
require fetching imports just to confirm that it wins. Static prefixes and filters
can prove a source cannot collide, avoiding unnecessary upstream dependencies.

Cache keys include environment, deployment/import configuration identity, and
effective credential identity/version; never use raw tokens as diagnostic keys.
Recheck authorization before using cached data. Successful refreshes replace the
view even when projection exclusions leave it empty. Refresh candidates include
imports used by running agents on historical deployments, not only the current
environment deployment. Schedule periodic work for active cached views; an evicted
view is fetched on demand. Reuse service cache/task conventions and document finite
limits and defaults in the service config rather than adding manifest knobs.

### Durable invocation and reflection

User clarification: the MCP bridge behaves as a special native tool whose remote
operation is an ordinary durable host call. Persist its result and replay it
without invoking the upstream server again.

Use the existing native-tool/entity machinery. Inside the bridge, a durable
remote-call boundary records the upstream response, including structured content,
errors, and payloads needed to reconstruct stdout. Commit that response before
publishing stdout or returning a result. A crash during subsequent projection or
stdout delivery must not repeat `tools/call`. Reuse normal stream recording and
settlement where required; do not add another MCP-specific stdout journal.
Native adapter and middleware reconstruction may execute normally while
the completed nested MCP operation returns its recorded response. Do not skip
middleware reconstruction or create a separate queue/oplog.

For metadata:

- Do not serialize full deployment-fixed native tool definitions into discovery
  oplog payloads. Reconstruct them from the exact deployment revision recorded
  in that observation, never from a new live selection.
- Persist dynamically observed MCP metadata, including reflective empty/absent
  outcomes and non-derivable merge inputs.
- Direct invocation must capture its own required dynamic metadata; a prior
  listing is not assumed.
- Preserve full projected metadata and required mappings, not just a digest,
  wherever replay or middleware needs them.
- Persist terminal resolution/validation rejections before exposing them to
  the guest. Transient infrastructure failures use normal retry handling.
- Completed replay must not fetch current upstream metadata or perform OAuth
  exchanges merely to reconstruct a recorded MCP result.

The discovery payload distinguishes no deployment, native resolution, imported
resolution, absence, and terminal error as appropriate to listing or lookup.
For lists, retain dynamic observations in declaration order, including empty
lists and exclusion/merge outcomes that are not derivable from fixed deployment
data. Rehydrate the native view and deterministically merge using the recorded
dynamic inputs. For direct invocation, capture its own selected import identity,
full projected metadata/mappings, and required middleware view at admission; never
depend on a preceding list. Refresh cannot swap this invocation's projection.
New invocations use the refreshed view; upstream rejection of already-dispatched
input follows the specified removed-tool/input-drift error mappings.

Expose exact-revision registry retrieval through the service/client boundary,
using the existing repository method. Cache immutable states by
`(environment, deployment revision)`, separately from the invalidated live
component-revision cache. Distinguish a missing revision from an existing empty
deployment. Accepted invocation activation already carries a deployment revision;
retain/reuse it, rehydrating only deployment-derived policy and metadata. Preserve
non-derivable principal, narrowing, filesystem, and dynamic middleware state.

Native metadata rehydration can require registry access if local caches are cold.
Treat transient retrieval failures as infrastructure retries, never as permission
to substitute the latest deployment. A missing exact revision or inconsistent
rehydrated activation is an invariant failure, not an empty result or fresh lookup.
Run retrieval retry handling outside the already-replayed durable call rather than
recording new discovery on replay. Upstream-offline replay and registry-offline
rehydration are different acceptance cases.

### Idempotency reuses existing HTTP/RPC techniques

User decision: remove the standalone idempotency prerequisite/refactor from the
plan. Reuse the existing durable identity derivation and outgoing HTTP policy.

- Send the Golem-derived `Idempotency-Key` header under the ordinary HTTP policy.
- Retrying one logical operation preserves its key; separate operations get
  distinct keys, including separate calls made by middleware.
- Preserve existing incomplete-call retry and atomic-region behavior.
- Prevent SDK/transport retries from bypassing that policy.
- JSON-RPC request IDs are correlation IDs, not deduplication keys.
- Completed calls replay without resending. Ambiguous incomplete external
  effects are deduplicated only if the upstream honors the key.

The MCP core specifications inspected during planning do not define a
deduplication-key mechanism. [SEP-3182](https://github.com/modelcontextprotocol/modelcontextprotocol/pull/3182)
proposed one but was closed without merging. Do not implement that proposal as
an adopted standard. Recheck the supported protocol's contract when implementing.

The Oracle raised an entity-context derivation concern, but no reproducer was
run. It is not an established bug or justification for a general refactor.
Verify the MCP integration with stable-key recovery tests and address only a
demonstrated gap.

### Authentication follows the original specification

The user explicitly requires following the spec, not choosing an alternative
environment-owned versus application-user credential model.

§5.7.2 specifies:

> Inline `bearer` references resolve through the manifest's value-substitution
> mechanism (e.g. `${WEATHER_TOKEN}` reading from environment or secret store).

> For OAuth flows that require an interactive consent step, `securityScheme` is
> the canonical path; the runtime caches tokens per the security scheme's
> configuration.

> [The bridge] reads that agent's per-tenant credentials and quota state out of
> the calling-agent context.

Therefore implement import-configured authentication, resolved under the calling
agent's existing context and narrowing rules. Follow the same account/environment
ownership and accounting model as other tools. Do not automatically forward an
ingress bearer token or invent application-end-user credential switching.

In the current runtime, the calling agent's account is its component/environment
owner account (also used for quota accounting), not the external requester or the
operator authorizing OAuth. Derive the grant's credential-owner account from that
environment on both operator and runtime paths; record the authorizing operator
separately for audit. Otherwise a collaborator's grant would never resolve for
the environment's agents. Operator authorization/disconnect require the existing
security-scheme `Update` permission. Trusted internal runtime resolution follows
the agent-secret lookup precedent and must not require administrative scheme
`View`; the live bridge still enforces the owning agent's effective tool/network
permissions and quota context. The operator identity is audit information, not a
replacement runtime principal or a requirement to retain administrative access.

The spec leaves outbound OAuth grant acquisition/storage details incomplete.
Implement consent, token storage/refresh, revocation, and cache/session isolation
consistently with the configured security scheme and caller context. Do not
silently defer OAuth or assume existing inbound authentication already implements
outbound OAuth.

#### OAuth implementation workflow

Implement a control-plane authorization operation, exposed by the CLI, for the
configured import/security scheme. The authenticated operator selects the existing
account/environment/import context and follows a browser consent URL. Use the
configured provider and applicable MCP OAuth requirements, including state binding,
PKCE where required, callback validation, and resource/audience scoping. Reuse
existing authorization and secret-storage facilities, not the inbound token itself.

Operator commands are `golem api mcp-import authorize|complete|status|disconnect`
with a zero-based import index and optional `--revision`. Authorization returns
the selected revision and a browser consent URL, and prints an exact-revision
completion command. After consent, the operator forwards the complete callback
URL to `complete`; this workflow does not start a local callback listener.
Consent expires after ten minutes. Starting authorization replaces any existing
grant immediately. All four operations require scheme `Update`, and status
exposes only scheme/context and grant state, never tokens. Completion/disconnect
return the status committed by their mutation, without a fallible status reload.
Public status labels are `pending-consent`, `exchanging`, `granted`, `refreshing`,
`authorization-required`, and `revoked`. A concurrent deployment must not change
the source used to complete consent: use the revision printed by `authorize`.

Store grants/tokens securely under that scheme and credential owner, with explicit
environment/account isolation. A calling agent resolves the configured credential
through its existing context and narrowing rules. Do not introduce application-user
credential switching. A discovery context records the environment/account,
deployment/import identity, effective capability narrowing, security scheme, and
credential-owner reference resolved for an authorized operation. Background refresh
may reuse only that authorized context, rechecking grant validity and permissions;
it cannot substitute the environment owner's or another agent's credentials.
Control-plane warmup without a resolvable credential context is skipped with a
warning. Narrowed caller-specific views are created on demand, not precomputed
using broader privileges.

A missing OAuth grant does not prevent deploying valid import configuration.
The operator can deploy, authorize through the CLI, then refresh or invoke on
demand. Failed warmup must remain distinguishable from a successful empty list.

Acquire/refresh tokens through a shared control-plane credential service and
durable grant store, called only by live resolver/bridge operations. OAuth token
exchanges are outside the agent oplog; guest-observable tool outcomes are not.
Serialize refresh per credential across service instances and publish replacement
tokens atomically, using generation checks so revocation or a newer grant cannot
be overwritten by an old refresh. Treat rotating refresh-token exchanges as
potentially non-idempotent; do not blindly retry an ambiguous exchange.
Reauthorization is preferable to claiming guaranteed recovery a provider does not
offer. Account for actual network requests under the appropriate existing account
policy. Persist token references and necessary metadata, not plaintext credentials,
in diagnostic/durable payloads.

Missing/revoked grants produce an actionable authorization failure identifying the
import/scheme: a recorded, non-retriable rejection, not a successful empty list or
an unbounded transient retry. Invocation reports an existing
`RemoteInternalError` describing the required authorization operation; discovery
uses its normal terminal-error path. Reserve `Denied` for caller authorization
failure, not a missing upstream grant. Agents do not initiate interactive browser
consent while a tool call is pending. Completed MCP replay does not require a
currently valid upstream token. Implement disconnect and reauthorization operations,
invalidating affected sessions and caches without changing historical replay data.

#### HTTP accounting boundary

Each process charges requests immediately before its own dispatch. Registry-owned
discovery, refresh and consent traffic uses the credential owner's monthly HTTP
ledger, including when a collaborator authorizes the import. Runtime requests
carry the agent's captured effective permission surface and use the same canonical
network target as executor HTTP. Operator consent requires scheme `Update`, not
an additional agent Network grant. Denials do not consume quota; cache hits and
coalesced waiters do not charge a request that they did not dispatch.

The runtime surface must permit the actual OAuth token endpoint as well as the
MCP resource endpoint. A configured issuer does not broaden the agent's Network
grant. Document the necessary provider-host grant alongside import authorization
in the operator guidance. A permission or accounting rejection before a refresh
POST restores the unused token under the existing generation/status fence;
concurrent revocation or reauthorization wins. Network errors, response loss and
cancellation remain ambiguous and never release a potentially consumed token.

Registry traffic does not consume an executor per-invocation HTTP counter;
executor-dispatched `tools/call` uses both that counter and monthly accounting.
Registry discovery is bounded independently by service page/timeout/concurrency
policy. This accounting distinction and charging operator consent to the owner
are implementation choices to highlight in final review. They avoid forwarding
stale monthly balances or reconciling counts after a lost registry response.
Monthly admission reuses the registry's checked usage update and atomic additive
write; like existing connection admission/executor batching, concurrent checks can
slightly exceed the limit. Monthly exhaustion must retain its typed error through
internal RPC and become the existing executor monthly-budget suspension trap,
not an automatic transient retry or a permanent tool failure.

### Projection and transport implementation boundaries

- Cover the mappings required by §5.7.3 using Golem's existing schema graph,
  constraints, and JSON value conversion. Validate JSON Schema under its declared
  supported dialect (default 2020-12); unsupported constructs exclude the affected
  tool, not silently permissive validation. Resolve local references within the
  fetched schema, subject to depth/size limits; do not fetch external references.
- Keep deterministic name mappings. For distinct names within one import that
  collide after sanitization, exclude the ambiguous candidates with diagnostics;
  the earlier-import precedence rule does not choose between tools in one import.
  Reject invalid final identifiers after prefixing, without inventing aliases.
- Input member mappings must be reversible. Reject ambiguous mappings and preserve
  original upstream keys when constructing `arguments`. Metadata digests cover
  projection-relevant schema, mapping, and annotations.
- Use the repository's MCP SDK and the smallest transport adapter that supports
  per-call managed headers, network policy/accounting, and Golem-controlled retries.
  Do not assume that enabling an SDK client feature provides those guarantees.
- Implement and test the latest released Streamable HTTP revision selected for
  this feature. Maintain an explicit tested supported-version set; a manifest
  override outside that set is a configuration error, and omitted versions follow
  MCP negotiation. Never negotiate a version merely because the SDK names it.
  Protocol-version negotiation required by the import contract is not a reason
  to add unrelated legacy transports or compatibility shims.
- Cover JSON and SSE responses, pagination, session behavior where applicable,
  request timeouts, cancellation, and progress handling for supported versions.
  Sessions are disposable and isolated by credential context. Initialize or renew
  sessions only for live operations; no hidden SDK retry may repeat `tools/call`.
  Progress notifications are transport-local liveness information, not guest
  output or oplog payloads; they must not bypass the overall operation timeout.
- Advertise only implemented client capabilities. Sampling, roots, elicitation,
  and task orchestration are not new Golem features in this task. If a tool requires
  an unsupported callback or continuation, return a clear recorded failure rather
  than hanging, granting authority, or pretending it completed. OAuth consent is
  the explicit operator workflow above, not application tool-time elicitation.
- Apply bounded schema, pagination, response, decoded-content, and concurrency
  limits using service policy. Exceeding a bound yields a diagnostic/error, never
  truncation presented as a valid result. Select numerical defaults during scoped
  config implementation and cover both sides of each limit in tests.

## Current-code observations and dependencies

These observations describe the inspected checkout, not test results. Recheck
them when implementation begins, since related work is ongoing.

- `cli/golem-cli/src/model/app_raw/mod.rs`: `Mcp` currently has deployments only.
- `golem-common/src/base_model/tool.rs`: tool sources distinguish component and
  fixed native host tools; imported dynamic metadata needs a deliberate model.
- `golem-worker-executor/src/native_tool.rs`: the native catalog validates fixed
  definition/version/digest identity. Reuse its execution substrate, not a
  mutable catalog entry per upstream tool.
- `golem-worker-executor/src/durable_host/tool/mod.rs`: `get_all_tools_model` and
  `get_tool_model` currently persist complete discovered definitions.
- `golem-worker-executor/src/services/environment_state/mod.rs` and
  `golem-registry-service/src/repo/deployment.rs`: current tool selection uses the
  latest deployment containing a component revision. Exact-deployment repository
  lookup already exists; expose it for replay without changing live selection.
- `golem-worker-executor/src/durable_host/entity.rs`: accepted invocation payloads
  already carry activation data including deployment revision; reuse that reference.
- No general JSON Schema → Golem schema importer was found.
- Existing MCP authentication is inbound-facing; outbound OAuth is additional work.
- Root `Cargo.toml` pins `rmcp` 0.16.0 with server transport features. Client support
  needs a scoped workspace dependency-feature change under `adding-dependencies`.
- Complete middleware compilation/chain traversal is absent from this checkout.
  [GOL-39](https://linear.app/golem-cloud/issue/GOL-39) was in review and identified
  GOL-438/GOL-439 as discovery/chain-traversal follow-ups. Foundational import work
  can proceed, but full middleware acceptance depends on those features.
- [GOL-25](https://linear.app/golem-cloud/issue/GOL-25) overlaps deployment context
  and native dispatch. Coordinate shared changes; do not absorb its virtual-owner
  or external-invocation feature into GOL-36.

## Step-by-step implementation plan

Update the checklist and evidence here as work lands. A step is complete only
after its affected tests, Oracle review, and bug-finder loop have been adjudicated.
Generated artifacts accompany each contract change.

- [x] **1. Expose exact-deployment rehydration while preserving live selection.**
  Keep the existing owner-component-revision lookup and invalidation behavior.
  Extend registry gRPC/client and executor service with exact-revision retrieval,
  an immutable revision-keyed cache, and explicit missing-versus-empty handling.
  Preserve the revision returned with each coherent deployment snapshot. Test
  config-only redeploys (new live operations see N+1; earlier observations replay
  N), cold-cache rehydration, and missing-revision failure. No new worker pin,
  lifecycle rules, or CLI targeting choices.
- [x] **2. Add ordered import configuration and dynamic source models.** Extend
  manifest validation, deployment persistence, source/activation identity, diff
  and hashing, APIs, and generated artifacts. Keep configuration separate from
  mutable cached projections; keep the fixed native catalog unchanged.
- [x] **3. Implement shared projection/conversion.** Cover names, filters,
  precedence, exact JSON field mappings, schemas, documentation, annotations,
  errors, typed upstream results, mixed-content variants, and simple text/binary
  stdout. Specify unsupported cases and bounds explicitly; test projection
  independently of network behavior.
- [x] **4. Implement policy-aware MCP transport and authentication.** Inspect SDK
  customization before choosing an adapter. Support the selected full Streamable
  HTTP contract, per-call keys, cancellation, and network accounting without
  hidden retries. Implement bearer/basic and outbound OAuth as separately
  estimable work units, both in scope. Follow the OAuth workflow and protocol
  boundaries above. Add an in-process mock upstream and provider fixture; the
  bearer/basic path can unblock resolver/bridge work while OAuth is implemented.
- [x] **5. Implement registry resolution on demand.** Share resolution across
  listing, lookup, invocation preparation, cache warming, and refresh. Handle
  pagination, auth-scoped caching, coalesced misses, successful empty lists,
  failures, precedence, and diagnostics. Do not rely on warm caches.
- [x] **6. Separate fixed and dynamic discovery persistence.** Rehydrate native
  definitions from the compact exact-deployment reference recorded for the
  observation/admission. Record dynamic MCP observations, rejections, and
  invocation snapshots with full metadata required by replay. Reuse existing
  activation references; retain non-derivable state. Preserve normal durable-call
  sequencing and infrastructure retry behavior, without a new owner association.
- [x] **7. Connect the special native MCP bridge.** Reuse native execution,
  authorization, accounting, cancellation, and existing HTTP/RPC idempotency
  techniques. Persist/replay the nested remote result, including structured content
  and simple-output stdout, without repeating completed upstream effects.
- [ ] **8. Complete middleware integration.** On the middleware dependency
  baseline, apply universal and per-tool chains, supply recorded layer-appropriate
  metadata, and dispatch through runtime-minted underlying handles. Revalidate
  on refresh and surface incompatible drift without bypassing middleware.
- [x] **9. Add operator and codegen surfaces.** Manual refresh, periodic host
  refresh policy, inspection, warnings, projected-metadata codegen consumption,
  and documentation. These use the same resolver rather than independent paths.
- [ ] **10. Verify the combined behavior.** Run focused projection, registry,
  executor, authentication, and CLI tests, then broaden across affected shared
  contracts. Update durability guidance and all required generated artifacts.

Ordering: step 1 must precede removal of fixed metadata from oplog payloads in
step 6. Steps 2–5 establish the contract, projection, client, and resolver consumed
by steps 6–7. Bridge work can proceed before native snapshot removal, provided MCP
dynamic metadata is already durably captured. Step 8 depends on the middleware
baseline; it is required for complete delivery. Step 9's refresh and authorization
CLI operations can be developed with their owning service steps. Step 10 adds
cross-cutting verification; every earlier step must already have run its own
affected tests. No standalone idempotency infrastructure step is added.

## Decisive acceptance cases

- Cold/evicted-cache listing and direct invocation without a prior list.
- Empty upstream list distinguished from cache miss and network failure.
- Native/import/import collisions and pre-prefix filtering.
- Upstream addition, removal, and schema changes after deployment.
- Invalid or unrepresentable definitions exclude only affected tools, with precise
  diagnostics; a newly unrepresentable definition does not retain its old live
  entry, while historical replay remains valid.
- Refresh between listing and invocation and during execution.
- Config-only redeploy with the same component revision: new live calls follow
  existing invalidation; recorded observations still rehydrate their old revision.
  A concurrent invalidation cannot mix tools, bindings, and imports from different
  deployment snapshots. Existing lifecycle/update semantics remain unchanged.
- Native definitions absent from discovery oplog payloads; dynamic MCP metadata,
  including absence, faithfully replayed.
- Completed MCP invocation replay with upstream unavailable, without discovery,
  token refresh, or another `tools/call`.
- Registry rehydration failure handled separately from upstream availability.
- Recorded no-deployment and empty-deployment results remain distinct. Missing
  historical revisions fail rather than silently reselecting or becoming empty.
- Lost-response recovery preserves the same key; separate calls have distinct
  keys. Count upstream effects, not just equal responses.
- Non-idempotent mode and atomic rollback follow existing durable policy.
- Structured results, tool/protocol errors, mixed-content values, and simple-output
  stdout replay correctly.
- Simple text/binary tools expose stdout; generated clients consume mixed output
  as typed values rather than parsing a content envelope from stdout.
- Declared upstream output types remain typed and coexist with accompanying
  content. The advertised result/stdout schema remains stable for tools whose
  returned content varies between calls.
- Empty, single-text, single-binary, multiple-block, resource-only, and simultaneous
  structured/stdout responses decode through generated clients with the same
  advertised schema. Invalid declared output is rejected, not defaulted.
- Output attachments settle even when no bytes are produced; cancellation and
  backpressure do not deadlock completion or cause a second upstream invocation.
- Crash after the nested response commit but before/during stdout delivery replays
  bytes without a second upstream call. Error responses bypass success-schema
  validation and replay the same string error. Raw calls without stdout attachments
  do not receive simple payload bytes; generated-client attachment paths do.
- Credential, cache, session, and quota isolation under caller context; OAuth
  consent/refresh/revocation without secret leakage.
- Universal and per-tool middleware receive coherent metadata; local effects
  reconstruct while completed upstream effects do not repeat.
- Multiple underlying calls, cancellation, and incompatible middleware drift.
- Imported metadata works with the existing typed-client generation path.
- OAuth missing grant, consent completion, token expiry/rotation, ambiguous refresh,
  revocation concurrent with refresh, and cross-context cache/session isolation.
- Deploy without an OAuth grant, then authorize and invoke from a cold cache.
- Per-name middleware binding precedes an upstream tool's appearance; it becomes
  usable only after discovery and compatibility validation, with no bypass.
- An unavailable earlier import with no cache never lets a colliding later import
  win. Partial pagination failures never publish partial listings.
- Unsupported protocol versions, callbacks, malformed responses, and resource
  limits produce explicit bounded failures; no silent schema weakening/truncation.

## Implementation choices and completion gates

The sections above settle behavior sufficiently to implement and test it. Exact
Rust/module names, CLI command spelling, cache tuning values, and the tested SDK
version are engineering choices to record with their owning steps. Consult the
original specification and current code first; escalate only an actual contract
conflict or unsupported prerequisite, not ordinary implementation detail.

- **Reference gate:** each durable observation/admission retains the deployment
  revision from the same snapshot as its tools, bindings, and imports. Prove exact
  rehydration before removing full fixed definitions; replay never calls the live
  latest-containing-component-revision lookup.
- **Typed-client gate:** exercise the output contract end to end in generated
  consumers, including optional stdout and the streamed/blocks branches.
- **Transport/auth gate:** verify the supported protocol set and no hidden retry
  behavior with the selected SDK. OAuth is required, not a silent future task.
- **Middleware gate:** integrate the real middleware discovery/chain baseline;
  stubs or schema fields alone do not satisfy the feature.
- **Review gate completed:** Oracle approved with amendments; those amendments
  and the focused deployment-reference follow-up are recorded below. Review
  approval is not execution/test evidence.

## Implementation evidence

### Step 1 — completed

- Added exact-revision registry repository/service/gRPC/client retrieval and the
  executor's separate bounded immutable revision cache. Existing live lookup and
  invalidation stay unchanged; full live state shares the same snapshot identity.
- Missing revisions are distinct from valid empty deployments. Cancelled callers
  cannot strand a cache fill. Responses for the wrong revision are rejected before
  caching; errors do not poison later retrieval.
- `cargo test -p golem-worker-executor --lib -- services::environment_state
  --report-time`: **22 passed**.
- `cargo test -p golem-registry-service --test tests --
  test_deployment_tool_snapshot_and_rollback --report-time`: **3 passed**
  (SQLite, PostgreSQL, PostgreSQL TLS).
- `cargo check -p golem-registry-service`, `cargo check -p golem-service-base`, and
  `cargo check -p golem-worker-executor`: passed. Package-scoped `cargo fmt --check`
  and `git diff --check`: passed.
- Oracle approved the production changes, requesting disposition of the bug-finder
  reproducer. Its recommended revision check was adopted.
- Bug-finder run 1 found a mismatched-response revision could enter the exact
  cache; accepted and fixed, with a retained regression test. Run 2 confirmed it
  resolved, with no new or recurring findings. Stopped the converged loop.
- These tests validate retrieval/cache behavior, not oplog replay changes. Native
  snapshot removal and end-to-end replay rehydration remain step 6.

### Step 2 — completed

- Imports are request-carried ordered deployment configuration, separate from
  MCP export staging and the fixed native catalog. Staged identity mirrors the
  current deployment's import hashes, as for existing request-carried tools.
- `McpImportDeployment` is the secret-bearing write input; `McpImport` is a
  secret-free descriptor. Credentials are stored separately in the same atomic
  deployment transaction and read only through a dedicated internal repository
  operation. Public plans/summaries carry index/hash identities, while tool
  snapshots carry complete descriptors. Dynamic source identity is environment,
  deployment revision, import index, and original upstream name; dispatch wiring
  remains in the bridge step rather than adding a nonfunctional source arm now.
- Inline credentials use the current manifest `{{ VAR }}` substitution mechanism;
  the historical `${VAR}` example does not introduce another interpolation engine.
  Missing variable errors identify the field without printing its credential
  template. Import order, auth identity, filters, prefix, and protocol override
  contribute to the shared CLI/server deployment hash (diff model v7).
- Credential identities are domain-separated, environment-scoped, length-prefixed
  BLAKE3 digests for change detection, not password hashes. Deterministic deployment
  hashes can permit offline guessing of weak inline passwords by deployment
  viewers. Use high-entropy upstream credentials. At-rest credential protection
  follows the registry's existing secret-column storage model.
- Oracle required fixes to staged/current import identity and import-only text
  plan output; both are implemented. Unified YAML diffs also include imports.
  Empty Basic fields remain permitted because Basic authentication does not itself
  prohibit them; transport-specific bearer validation belongs with step 4.
- Bug-finder run 1 reproduced acceptance of an endpoint containing an embedded
  ASCII control character. Accepted and fixed with an explicit rejection before
  URL parsing; the reproducer is retained. Run 2 confirmed it resolved with no
  new or recurring findings; stopped the converged loop.
- `cargo make generate-openapi` completed and regenerated REST reference MDX;
  `cargo build -p golem-client` passed. The combined registry/client/CLI
  `cargo check --all-targets` passed after updating a remaining deployment
  constructor in the remote-release integration test.
- Targeted common metadata/protobuf/fingerprint tests: **9 passed**; MCP model
  follow-up: **5 passed**; registry HTTP validation: **1 passed**; expanded
  deployment repository test: **3 passed** (SQLite, PostgreSQL, PostgreSQL TLS);
  CLI MCP/output-schema tests: **22 passed**; raw manifest module: **23 passed**;
  executor environment-state module: **22 passed**.
- Native-tool deployment regression: **1 passed**. Remote-release CLI integration
  regression: **1 passed**. Built the missing Rust streaming fixtures through the
  CLI and retained their SDK dependency lockfile update; built the single-binary
  server and TypeScript guest runtimes before the CLI regression. An initial
  test build exhausted disk space; the rerun passed after clearing completed
  build caches and disabling incremental compilation.
- Package-scoped format checks and `git diff --check`: passed.
- The later live credential resolver must resolve the descriptor first: an absent
  optional inline credential is valid for anonymous/security-scheme imports, but
  is not a substitute for a missing import or a missing required inline secret.

### Step 3 — completed; budget boundary remains provisional for final review

- Projection lives in the host-only `golem-mcp-import` crate, shared by the later
  registry and executor paths without introducing a JSON Schema validator into
  guest SDK dependencies. Serializable projections retain their original schemas
  and admission-time limits; compiled validators are lazily cached, not persisted.
- Original JSON Schema 2020-12 validation remains authoritative in both conversion
  directions. Golem metadata represents structure and documentation; validation-only
  constraints remain enforced even when Golem cannot describe them directly.
  Local JSON Pointer references, recursive containers, tagged record unions,
  nullable choices, fixed tuples, and structural conjunctions are supported.
- Default integers use signed 64-bit values, or unsigned 64-bit values for
  nonnegative domains. Explicit bounds can select smaller widths. Out-of-domain
  bounds do not prevent admitting representable values. Conversion rejects
  overflow and lossy integer-to-f64 conversion rather than saturating or rounding.
  This is an explicit numeric representation limit, not arbitrary-precision JSON.
- Optional nullable members use `option<record { value: option<T> }>` to preserve
  absent versus explicit null without illegal nested nullable types. Unspecified
  additional values use Golem's existing JSON-string convention in an
  `additional-properties` map. Pattern-matched keys are preserved in that map and
  checked against every applicable original constraint. A root object union uses
  one required, typed `arguments` option rather than flattening away its branch.
- Unsupported dialects, external/anchor/dynamic references, nested reference-scope
  changes, ambiguous non-null type unions, untagged record unions, non-fixed tuple
  shapes, and conflicting synthetic/member names exclude the affected tool with
  diagnostics. They do not invalidate other definitions in a successful listing.
- Default limits: 1 MiB / 16,384 nodes / depth 128 for a definition or schema,
  4 MiB / depth 256 for an instance, 1,024 tools / 8 MiB for a listing, and
  256 blocks / 8 MiB encoded / 4 MiB decoded / depth 32 for content. Expanded
  conjunctions and typed values are checked before recursive conversion.
  Schema/reference depth 128 and instance depth 256 are supported hard ceilings;
  callers may lower them. Validation-only reference paths are checked before
  compiling the validator. Conversion collapses aliases and uses a temporary
  64 MiB native stack when the caller's stack is smaller.
- Oracle review prompted nullable multi-branch union and conjunction/reference
  coverage, cached validators, stable digests across limit changes, and the
  existing unit-tuple convention. Bug-finder run 1 identified digest churn from
  ignored upstream extension fields; the digest now covers the upstream name,
  original input/output schemas, and projected tool definition only.
- Bug-finder run 2 confirmed the digest fix and identified omitted rejected
  definitions in listing-byte accounting. The full listing is now counted
  iteratively, including over-deep/invalid definitions, before per-tool exclusion.
- The follow-up Oracle stack-overflow reproducer failed on a 2 MiB thread before
  the fix. Ceiling regressions with 124 annotated aliases or 62 conjunction hops,
  each across 255 nested objects, now pass, including Golem schema-value validation
  and both conversion directions. They also passed with the conversion stack
  temporarily reduced to 32 MiB; the implementation retains 64 MiB. Over-deep
  validation-only reference chains are rejected. Oracle confirmed resolution and
  no recursive-container regression. Reference sites inherit nearest available
  documentation, including terminal-definition documentation, intentionally.
- Bug-finder run 3 confirmed the listing fix but found the whole response charged
  against the content-only budget. Its third successive new finding triggered the
  mandatory design checkpoint; work paused until the user's explicit approval.
- Provisionally approved boundary (must be raised again in the final review):
  transport owns the total wire-response budget;
  projection checks structured content against the instance budget and content
  blocks against their encoded/decoded/count/depth budgets independently. Response
  wrapper fields must not consume a content-only budget. Apply this consistently
  to declared and undeclared structured content and bounded error rendering, while
  preserving `isError` bypass of successful-output schema validation. The shared
  issue is conflating complete upstream envelopes with admitted metadata and
  separately projected payloads, rather than an incorrect limit constant.
- Checkpoint verification: `CARGO_INCREMENTAL=0 cargo test -p golem-mcp-import
  --lib -- --report-time` reported **34 passed, 1 failed**. The retained regression
  test is `response_does_not_charge_wrapper_and_structured_content_to_content_limit`.
  Strict all-target Clippy passed before that reproducer was added. The user then
  approved the proposed boundary for continued implementation, explicitly asking
  that it be mentioned at the end for final review. This authorization permits
  resuming the loop with the material boundary revision recorded in the override.
- Projection now checks declared and undeclared structured instances independently
  from content, and error rendering enforces content count/depth/encoded bounds
  plus the rendered string's decoded-byte budget. It does not validate ignored
  successful-output fields on `isError`. The later transport must bound the full
  response before parsing or invoking projection, including wrapper/extension data.
- Final verification: **37 tests passed**, all-target check and strict Clippy
  passed, scoped formatting and diff checks passed. Oracle found no blockers in
  the revised boundary. Bug-finder run 4, authorized by the user after the design
  checkpoint, confirmed the remaining finding resolved and found no new issues.
  All findings are adjudicated; the converged loop is stopped.
- Carry to steps 9–10: exercise actual generators with both omitted and present
  optional arguments and extras. Oracle identified differing pre-existing SDK
  optional-carrier conventions; strict canonical-value tests alone do not close
  that integration gate.

### Step 4 — completed

The chronological evidence below records intermediate gates. Final transport/auth
acceptance and the closing Oracle/bug-finder results are recorded under step 10.

- Official live versioning now identifies **2026-07-28** as current. Imports
  support only that released revision: self-contained request metadata, required
  method/name/parameter headers, JSON or request-scoped SSE, and no protocol
  sessions, initialize handshake, GET stream, DELETE, or SSE resumption. Omitted
  versions select it; an override outside the tested set is rejected before
  traffic. There is currently no second tested revision to negotiate. References:
  [versioning](https://modelcontextprotocol.io/specification/versioning) and
  [Streamable HTTP](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http).
- Upgraded the shared `rmcp` dependency to released 3.4.0. Oracle's SDK inspection
  found that the full client worker cannot preserve invalid-definition isolation,
  raw results, explicit fetch failures, and admission-time header mappings without
  side channels. Its typed listing parsing, internal metadata cache, automatic
  continuation handling, and retry machinery conflict with these requirements.
  Use SDK wire models/metadata and the SDK's SSE parser behind a direct bounded
  POST sender instead. This is the smallest adapter after inspecting the SDK,
  not a parallel implementation of legacy transports.
- `ProjectedTool` now persists validated `x-mcp-header` paths from the original
  input schema, including nested properties. Calls extract original argument
  paths, enforce safe integers, and encode unsafe/sentinel-looking header values.
  Invalid annotations exclude the individual definition during projection.
- The transport retains whole raw results and paginated raw definitions, with
  no projection-time metadata fetch. Every call performs one POST; retries remain
  with the later durable boundary. Bearer/basic headers are validated and marked
  sensitive. Request IDs remain correlation IDs, not idempotency keys.
- Initial numerical transport defaults: 8 MiB request, 16 MiB response/event and
  cumulative listing wire bytes, 64 pages, 1,024 tools/notifications, JSON depth
  at most 512, 16 concurrent operations per shared client, and a 60-second total
  deadline including permit acquisition and all pages. SSE framing/comments count
  toward the same total response budget before the SSE parser sees them. These
  limits are independent of projection budgets; service policy wiring remains.
- Oracle identified and prompted fixes for the upgraded export service's new
  loopback-only Host filter, transient SSE/gateway failures misclassified as
  terminal protocol errors, and the protocol-mandated default of `complete` when
  `resultType` is absent. Preserve the export service's existing protocol range
  and absence of an SDK-level request-size cap, rather than broadening its
  capabilities or imposing an unrelated new 4 MiB limit during the import work.
  Import response bounds remain enforced. The customer-domain regression test
  exercises the real HTTP service adapter, beyond the export projection tests.
- Bug-finder run 1 reproduced case-sensitive handling of `Content-Encoding:
  Identity`. Accepted and fixed with a retained regression. Run 2 confirmed it
  resolved and reproduced an ordinary JSON-RPC error accepted without a request
  ID. Fixed ordinary-error correlation while retaining the protocol's explicit
  omitted-ID exception for malformed-request parse/invalid-request errors.
  Cloned clients also share an increasing request/progress ID allocator so
  concurrent in-flight operations never reuse the same ID. Both sides of the
  exception and concurrent calls are covered by tests.
- Oracle's follow-up found no blockers in the foundation and verified export
  protocol parity. Its encoding/status ordering hardening was also applied:
  request identity encoding explicitly, preserve authorization/HTTP status errors
  without decoding compressed error bodies, and reject compressed successes.
  Production senders must not introduce transparent unbounded decompression.
- Bug-finder run 3 confirmed the correlation fix but found that valid surrounding
  HTTP whitespace in `Content-Encoding: identity` was rejected. The retained
  regression test is
  `transport::tests::accepts_optional_whitespace_around_identity_content_encoding`.
  Fingerprint: `transport-content-encoding-identity-optional-whitespace`.
  Three successive new findings triggered the mandatory design checkpoint.
- Checkpoint assessment: two findings share raw HTTP token normalization as their
  root cause; the request-ID finding is a separate, now-fixed envelope invariant.
  The user approved a library-first revision: existing `headers::ContentLength`
  validates repeated length fields; `headers::ContentType` and `mime` parse MIME
  syntax, with an explicit singleton check. The typed encoding API cannot expose
  all codings, so a small identity-only check traverses every repeated field and
  comma-separated token, normalizes case/OWS, and ignores empty list members.
  Compression cannot hide beside an identity token. JSON-RPC correlation remains
  independent. No new transport framework or tool behavior was introduced.
- Oracle approved this revision without blockers. Bug-finder run 4, authorized by
  that user decision, confirmed the OWS finding resolved with no new findings.
  All reported transport findings are now resolved. Its returned status still
  had `designCheckpoint: true`; the user subsequently explicitly approved
  continuing beyond that checkpoint to the remaining step-4 implementation.
- Current verification: import suite **59 passed, 0 failed**, including repeated
  coding fields/lists, mixed case/OWS, conflicting lengths, overflow, and MIME
  parameter validation. Scoped formatting and strict import Clippy passed.
  Earlier expanded MCP export suite **105 passed**, including the real HTTP
  customer-domain regression, and combined import/worker-service all-target checks
  passed. A disk-full build failure was recovered by clearing superseded build
  caches, without removing source or guest fixtures.
  Step 4 remains **not complete**; the reviewed foundations are a separate local
  implementation checkpoint, not completion of transport/authentication integration.
- The production HTTP sender now disables reqwest retries (including protocol
  retries), redirects, automatic decoding, proxy discovery, and Referer synthesis.
  Admission runs immediately before each request; streaming response conversion
  does not buffer the body. Sender errors preserve host-specific quota traps.
  The executor policy adapter reuses live network authorization, per-invocation
  counting, and monthly accounting; it rejects replay dispatch before consulting
  current authority. Actual bridge invocation remains step 7.
- Outbound grant persistence uses the existing private SQL secret-storage boundary.
  Grants are scoped by environment, scheme identity/revision, credential-owner
  account, and canonical resource URL, not by ingress bearer tokens. Consent and
  refresh claims must be durable before exchanges; generation/state compare-and-set
  prevents concurrent refresh or late exchange completion from undoing revocation
  or reauthorization. An abandoned in-flight exchange requires reauthorization,
  not a lease expiry that can repeat a rotating-token exchange.
  Every refresh claim also rotates the generation: a retained old grant cannot
  start another refresh after its peer completes, and a late completion cannot
  overwrite a subsequent cycle. This was reproduced before fixing and is covered
  by `sqlite_refresh_claim_fences_subsequent_refresh_cycles`.
- The expanded import suite is **65 passed**, including real HTTP pagination,
  per-request policy checks, quota-error propagation, raw encoded streaming,
  redirect/auth-status refusal, response-loss no-resend, and timeout closing the
  server response stream. Strict import Clippy and scoped formatting pass.
  Executor library compilation passes (three unrelated pre-existing warnings).
  The OAuth store's **3 SQLite tests pass**; PostgreSQL migration execution remains
  an integration check, not yet verified. Oracle reviewed both foundations with
  no blockers; bug-finder transport run 5 and separate OAuth-store run 1 each
  returned **no bugs found**, clean terminal results with no checkpoint gate.
- OAuth service choices: explicit reauthorization immediately invalidates the old
  grant. Other live callers wait with a finite deadline while refresh is underway;
  they cannot acquire its old token or repeat its exchange. Failed exchanges are
  conservatively treated as ambiguous, including connection failures. The native
  handler owns `&mut Ctx`, so its internal durable live arm can use the current
  adapter; an accessor-side implementation would instead need serialized access.
- OAuth protocol helpers now implement RFC 9728 protected-resource discovery,
  challenge-first/path/root fallback, ordered RFC 8414/OIDC issuer discovery,
  exact resource/issuer binding, advertised S256 verification, and the RFC 9207
  callback issuer matrix. Discovery URLs use the RFC trailing-slash transformation
  without normalizing the recorded issuer; resource queries are preserved.
  `oauth2` constructs PKCE, authorization, code and refresh requests. A private
  client binds validated issuer/callback/resource; both token requests include
  the resource indicator and use the policy-aware single-attempt sender.
- `http-auth` parses challenge boundaries and quoted escapes; the SDK's helper
  only searches parameter substrings and cannot safely select Bearer challenges.
  Ambiguous/malformed challenges fail closed, including unsupported token68
  challenges; no hand-written alternative parser is introduced. Metadata fallback
  is limited to 404; malformed successful metadata or binding mismatch is terminal.
  OAuth requires HTTPS resources/providers; loopback HTTP is permitted only for
  the operator callback. Pre-registered security-scheme clients are the supported
  registration path, with Basic, POST-secret, or advertised public authentication.
- OAuth defaults bound each document to 1 MiB, request body to 64 KiB and challenge
  headers to 16 KiB, with one 20-second discovery-chain deadline. Token exchanges
  also bound response streaming and duration. No redirects, decoding, or retry is
  added. Malformed responses, client configuration failures, and rejected grants
  have distinct diagnostic categories; none permits retrying a claimed exchange.
  Provider error descriptions, extension error codes and malformed token bodies
  are never rendered; host quota/authorization errors retain their types.
- Oracle requested error classification changes; implemented and confirmed with
  no remaining blockers. Its follow-up also prompted handling the RFC-required
  HTTP 401 `invalid_client` response and documenting refresh failure policy.
  Separate OAuth-protocol bug-finder run 1 returned **no bugs found**, clean
  terminal with no checkpoint. Full import suite **81 passed**; strict all-target
  Clippy, scoped formatting and diff checks passed. These are protocol fixtures,
  not end-to-end operator or registry integration evidence.
- Callback integration must persist the validated issuer metadata (including the
  `iss` requirement and token authentication method) with state/PKCE and rebuild
  from that record, never rediscover on callback. Validate `iss` on error callbacks
  before interpreting provider errors too. The serializable metadata accessor
  supports this; shared-store claiming and effective-context rechecks remain with
  the registry credential service.
- The registry OAuth coordinator now resolves the exact deployed import and its
  current scheme revision, distinguishing anonymous imports, inline credentials,
  missing imports and missing grants. Operator operations use scheme `Update`;
  runtime resolution uses a trusted owner context without admin `View`. Private
  session data persists the validated provider metadata, scopes, deployment/import
  reference and authorizing actor through token publication for later refresh.
- Authenticated operator callback completion claims hashed state before the token
  POST, rechecks actor/authority/context, validates issuer on success and error,
  and rebuilds the client from saved metadata. Scheme changes and grant generation
  changes fence token publication. Explicit reauthorization supersedes prior state;
  disconnect and late completions cannot undo one another.
- Refresh waiters use bounded backoff without repeatedly loading the complete
  deployment. A successful refresher returns its published credential directly.
  Missing/revoked grants and unresolved refreshes carry the scheme name and an
  explicit reauthorization action. `RefreshUnresolved` must become a recorded
  terminal rejection in the bridge, never an automatic transient-retry loop.
- Oracle confirmed the ownership and claim/publication boundaries. Its requested
  changes separate an impossible owner mismatch from context changes, remove
  deadline failures after successful publication, and make abandoned refreshes
  actionable. A pre-POST context read now precedes the refresh claim, so its
  failure does not consume an otherwise usable refresh token.
- Cancellation is deliberately treated like ambiguous response loss: no lease
  expiry or token reuse. The coordinator leaves a cancelled claim fenced; future
  operations wait finitely and require explicit reauthorization. Oracle suggested
  best-effort asynchronous cleanup to shorten that wait, but it would still
  require reauthorization and cannot cover crashes. The simpler abandonment
  behavior is explicit and tested by cancelling after dispatch, verifying no
  resend, and recovering via a new consent flow. This can make an interrupted
  refresh require operator action, not just a service crash.
- Coordinator/store tests: **13 passed**, including collaborator ownership,
  saved-metadata/PKCE binding, callback replay/denial, scheme rotation and revoked
  authority, exact-deployment lookup, concurrent refresh, response loss,
  cancellation, refresh replacement/fallback and expiry boundaries. Bug-finder
  run 1 exposed an unexpired stored token gated by the refresh timeout; retained
  the regression and fixed the deadline boundary. Run 2 confirmed it resolved,
  all 13 tests passed, and no new findings. The fully adjudicated loop is stopped.
- Full import suite: **81 passed**. Registry all-target strict Clippy with
  `--no-deps`, scoped format checks and `git diff --check` passed. The initial
  dependency-inclusive Clippy attempt exhausted disk; after clearing obsolete
  build caches it reached an existing `collapsible_if` lint in
  `golem-common/src/base_model/mcp_import.rs`. Package-only linting is clean; the
  broader lint remains for final validation rather than being silently suppressed.
- Consent now owns the unauthenticated probe: one credential-free `tools/list`
  POST to the exact deployed endpoint, using the shared request builder. Only
  bounded `WWW-Authenticate` values from 401 are used; 200 supplies no challenge,
  other statuses fail without retry, and no probe body is consumed or advertised
  as tool metadata. Probe plus metadata discovery share one deadline. HTTPS and
  protocol validation run before traffic, using the same OAuth URL validator as
  discovery. A failed probe preserves the existing grant.
- Probe verification: the request-builder extraction first passed all **81**
  existing import tests before adding behavior. The expanded import suite now
  passes **83 tests** and registry OAuth/store suite **16 tests**. Both packages'
  all-target strict Clippy with `--no-deps`, scoped format and diff checks pass.
  Oracle found no blockers; applied its shared-validator and pre-traffic test
  suggestions. Separate probe bug-finder run 1 returned **no bugs found**, clean
  terminal with no checkpoint. Production request-policy wiring remains untested.
- Registry HTTP admission now shares canonical network-target derivation with the
  executor. Runtime policy accepts only an agent surface bound to the environment
  owner, checks each actual target before charging, and preserves typed monthly
  quota errors. Operator policy requires scheme `Update` and bills the environment
  owner, including collaborator consent. Cache authorization is non-charging.
- Oracle confirmed the dispatch-local accounting design and the implementation.
  Its refresh follow-up is implemented using the existing `publish_refresh` CAS:
  known permission/accounting rejection restores unused tokens without undoing
  concurrent revocation. Ambiguous failures and cancellation remain fenced.
  Provider network permission is intentionally required for runtime token POSTs;
  the operator guidance must explain this, rather than silently broadening grants.
- Verification: **4** shared normalization tests, **17** OAuth/store tests, and
  **6** policy/accounting repository tests passed (SQLite, PostgreSQL, PostgreSQL
  TLS). Executor library check passed with three existing warnings; registry
  all-target strict Clippy (`--no-deps`), scoped formatting and diff checks passed.
  Bug-finder returned **no bugs found**, clean terminal with no checkpoint.
  Disk-full linker and later PostgreSQL setup failures were recovered by removing
  obsolete build caches; the final unchanged DB tests passed on all three variants.
  PostgreSQL migrations now execute successfully, but grant-state transition tests
  still run only against SQLite. No production endpoint is wired yet.
- OAuth limits now have a registry config surface with human-readable duration
  serialization; numerical defaults remain in the shared library. Bootstrap
  validates before opening the DB/starting tasks, constructs the grant repository
  for both backends, and exposes the configured coordinator. The single-binary
  launcher inherits these defaults. No sender or endpoint is wired by this alone.
- Config/bootstrap verification: **22** registry config/OAuth tests and **83**
  import tests passed; scoped all-target strict Clippy (`--no-deps`), formatting,
  and diff checks passed. Registry binary build and single-binary launcher check
  passed. Generated the owning registry TOML/env files with the freshly built
  binary's two dump flags from `generate-configs`; a second dump matched exactly.
  Used scoped generation instead of rebuilding missing unrelated service binaries
  in the full task. A broad launcher all-target check exhausted disk; after
  removing obsolete test/build artifacts, the production launcher check passed.
  Oracle found no blockers and bug-finder returned **no bugs found**, clean
  terminal. Zero challenge bytes remains a literal header budget (rejecting
  nonempty challenges), not a new configuration-disable switch; the default is
  unchanged. General transport/projection policy wiring belongs with their
  resolver/bridge consumers.
- Operator REST and CLI operations are wired to the production accounting sender.
  Accounting is a required service dependency. Callback claiming is scoped to
  the path environment; wrong-environment attempts cannot consume another flow.
  Actor/deployment/import binding remains checked before token exchange. Public
  responses use camelCase and CLI outputs have registered typed discriminators.
- Oracle's two operator reviews found no blockers. Its suggested environment
  claim fence, post-mutation response simplification and exact-revision completion
  hint are implemented. Generated clients no longer log request bodies, preventing
  verbose CLI logging from exposing callback code/state. This intentionally removes
  body logging for other generated API calls too; request serialization is unchanged.
- Operator verification: **20** registry OAuth/API tests, **21** CLI callback/schema
  tests, and **1** generated-client logging regression passed. The real Poem route
  test exercises authentication, exact source lookup, callback rejection, and
  disconnect/status against SQLite; it completes in under one second and spawns
  no external processes. `generate-openapi` and `cargo build -p golem-client`
  passed; OpenAPI and REST MDX are regenerated. Formatting and diff checks passed.
  The dedicated operator bug-finder run returned **no bugs found**, clean terminal.
  This is not yet an end-to-end provider/agent test or completion of step 4.
- Runtime credential and resource-401 feedback RPCs now require the environment
  owner's Agent context and resource Network authority before exposing even
  cached/inline credentials. Refresh separately checks provider authority. The
  private client result redacts credentials; no credential enters an oplog.
- A 401 report expires only the generation used by that request, preserving its
  refresh token. Generation/status CAS prevents stale feedback from expiring a
  newer token or changing a revoked/refreshing grant. Quota failures retain the
  existing LimitExceeded category; unresolved grants are terminal BadRequest,
  while network failures, timeouts, 429 and 5xx remain infrastructure errors.
- Oracle confirmed authority, error categories and the generation fence. Its
  deadline finding is fixed: OAuth now defaults to 20 seconds, including operator
  operations, below the executor's 30-second registry request timeout. Wait and
  token exchange share that budget. Pre-dispatch exhaustion restores unused
  tokens under CAS; successful publication is not failed by a later timer.
  Generated registry TOML/env files also carry 20 seconds. Custom deployments
  must preserve headroom between OAuth and registry-client timeouts; the services
  cannot validate one another's configuration. The default test reserves >5s.
  Slow database operations can still exhaust client deadlines. A gRPC disconnect
  or request timeout after claiming refresh is cancellation and can require
  explicit reauthorization, just like the previously agreed interrupted refresh.
- Targeted registry OAuth/API/error tests: **24 passed**, including an in-process
  production gRPC/client round trip, denied contexts, stale feedback and a peer
  takeover that cannot restart the refresh budget. Bug-finder run 1 returned
  **no bugs found**, clean terminal with no checkpoint. Registry binary build and
  scoped config regeneration passed. Consumer regressions: **147 executor tests**,
  **153 worker-service tests** and **83 import tests** passed. Disk-full compilation
  was recovered by removing obsolete build artifacts. The new executor deadline
  test initially lacked two imports; its corrected version passed in that run.
- Remaining step-4 work: discovery/bridge consumption of resource-401 feedback
  and metadata-cache invalidation; provider fixtures and combined validation.
  The coordinator is an intermediate checkpoint, not completion of step 4 or
  evidence of end-to-end OAuth operation.
- Combined resolver/OAuth verification now uses an in-process HTTPS provider and
  resource with a test-owned CA. Consent, metadata discovery, code exchange,
  generation-keyed cache hits, resource-401 invalidation, refresh rotation, and
  stale-401 fencing execute through the production coordinator and resolver.
  A 401 performs no same-call refresh/retry; the next resolution refreshes once.
  `HttpSender` can add a trust root without changing retry/redirect/decoding policy;
  the service's fixture trust hook exists only in test builds.
- Oracle found the new fixture's challenge incorrectly applied to anonymous
  imports too. The challenge is now opt-in. The full registry MCP/OAuth suite
  passes **37 tests, none ignored**; sender tests pass **6 tests**. The dedicated
  HTTPS integration bug-finder loop confirmed the fixture fix and returned a
  clean terminal result on run 2. Bridge-side 401 feedback and full agent/provider
  acceptance remain outstanding; this does not close step 4.

### Step 5 — completed

- Registry listing, lookup, refresh and the internal observation RPC share one
  on-demand resolver. Complete pagination and projection precede publication;
  successful empty lists and per-definition exclusions replace old observations.
  Native names win without discovery; imported lookup stops at the first winner.
  Whole listings resolve all imports concurrently, then merge in declaration order.
  An unavailable import fails the listing rather than publishing a partial view.
- Cache identity includes exact environment/deployment/import, the complete
  effective authorization context, and inline-credential digest or OAuth grant
  identity/generation. Runtime authority is checked even for hits. No credential
  is included in the RPC observation. Defaults: 128 entries, 16 concurrent
  context/fetch operations, 25-second operation budget, 300-second success TTL,
  one-second failure TTL, and a 20-second transport budget. Config dumps are
  generated from the newly built registry binary.
- Coalesced fills survive caller cancellation, have bounded lifetimes, and retain
  their capacity permit through CPU projection. Pending entries are never evicted;
  saturation is explicit. Last-success metadata survives ordinary upstream,
  protocol, projection and infrastructure errors. Authorization, changed context
  and quota errors never fall back to stale data. Manual refresh reports failure.
- OAuth credential acquisition retains the agreed cancellation behavior. A resolver
  deadline can fence an interrupted refresh and require reauthorization. Oracle's
  suggested detached refresh is not adopted. Its separate sibling-cancellation
  finding was fixed: listing uses `join_all`, not short-circuiting `try_join_all`,
  so one failed import does not cancel another import's credential acquisition.
  A delayed-first/import-denial regression also proves ordered error reporting.
- Resource 401 clears that cache entry and reports the used OAuth generation,
  without retrying inside resolution. Combined OAuth generation/401/refresh/cache
  validation remains step 4 integration; the anonymous 401 test is not that proof.
- The RPC preserves typed quota, authorization, missing-source and invalid-input
  categories. A real RPC regression exposed serde's default depth rejecting a
  valid projected schema; internal snapshot serialization now uses the same
  stack-growth technique as projection. Upstream depth/byte limits remain intact.
- Targeted registry MCP/OAuth suite: **36 passed**. Registry binary build and
  scoped config generation passed. Oracle confirmed its blockers resolved.
  Bug-finder `gol36-step5-resolver` run 1 returned **no bugs found**, clean terminal
  with no checkpoint. Scoped lint subsequently identified representation-only
  fixes and a missing `mcp_imports` field in a step-2 service-base test constructor;
  those are corrected. Strict all-target/no-deps Clippy passed for registry,
  service-base and import. The affected service-base environment test passed;
  executor and worker-service all-target consumer checks passed.
- Availability tradeoff: the shared context/fetch semaphore can delay cache hits
  behind active fills. Auth-context keys deliberately do not merge distinct
  authority surfaces. Periodic/operator refresh surfaces and actual executor
  discovery/admission consumers remain their later implementation steps.

### Step 6 — completed

- Fixed discovery now records an optional exact deployment revision rather than
  native definitions. Four worker tests passed, including genuine executor
  teardown/restart, N-to-N+1 live deployment changes, missing historical revision
  failure without fallback, and no-deployment versus empty-deployment observations.
  Tests decode actual discovery oplog responses to prove fixed metadata is absent.
- Oracle reviewed the fixed scaffold and dynamic integration boundary. The bridge
  will remain a Host executable (`mcp-import@1`), with a full dynamic projection
  carried in its activation policy, not a mutable native catalog entry. All
  imports use the bridge's stable synthetic discovery component identity.
- Dynamic discovery is connected to the existing environment-state client.
  Ordered per-import metadata, empty lists and exclusions are persisted; native
  names (including unbound names) precede imports. Completed discovery replay
  uses those observations and the exact native deployment, without MCP/OAuth.
  The discovery module passed all **6 tests**, including dynamic restart, cold
  ordered lookup and fail-closed behavior. Native streaming and reordered
  admission/replay regressions passed **2 tests**; executor all-target check passed.
- Admission now selects one coherent deployment and synthesizes the MCP bridge
  binding/projection. The activation fingerprint and protobuf include the dynamic
  source and full projection; common-model validation checks bridge identity,
  deployment and owner environment. A separate bug-finder run returned a clean
  terminal result. Its retained tests cover monthly-budget suspension with an
  incomplete discovery Start and cold invocation reaching native bridge dispatch.
- Oracle found the discovery quota branch violated the unfinished-session drop
  contract and the reordered-admission interceptor still used the old lookup API.
  Both are fixed, and its follow-up confirmed resolution. Quota egress additionally
  uses the call-owned trap marker, covering ephemeral agents where quota exhaustion
  is an error rather than suspension.
- Combined discovery/admission verification now passes **8 tests**, including
  cold MCP invocation and genuine executor reconstruction after stopping upstream
  and clearing credentials/observations. The bridge makes no additional HTTP,
  credential, metadata, live-deployment or exact-deployment lookup on completed
  replay. The earlier failing test called an uninitialized guest getter; its first
  correction accidentally added discovery, whose exact-revision replay lookup is
  required. The final test invokes cold and triggers recovery through the existing
  self-metadata method instead. Log: `/tmp/gol36-bridge-discovery-fixed2.log`.

### Step 7 — completed

The chronological evidence below records intermediate gates. Final quota, OAuth,
public-oplog and config acceptance closes this step; middleware remains step 8.

- Native dispatch recognizes the MCP activation and uses the existing native
  context, retained entity state and attachments. A nested `WriteRemote` call
  derives the ordinary HTTP/RPC key on both live and replay, records the complete
  transport result, and commits it before projecting values or writing stdout.
  Credentials and exact endpoint lookup occur only in its live arm. Resource-401
  feedback follows the committed response and never resends that call.
- Projection decoding checks the binding name/digest/upstream identity on a
  blocking stack; typed results use the recorded result schema and ordinary custom
  errors use the declared string payload. Cancellation records its outcome inside
  the remote boundary; attachment failures are arbitrated by the owner operation.
- First Oracle review found no blocker in the nested durability/key/replay design.
  Accepted follow-ups preserve exact quota traps, classify immutable snapshot
  failures and unify admission/credential error mapping. Its suggestion to map
  missing consent to `Denied` conflicts with the settled contract above: missing
  upstream grants remain `RemoteInternalError`, while caller denials use `Denied`.
  No compatibility encoding is added; the repository permits format replacement.
- The initial HTTP/restart test build exposed missing imports, a test error
  constructor mismatch, and use of Poem in an Axum-based test crate; corrected.
  Production compilation and completed-call restart now pass (see step 6).
  Stdout/crash, lost-response, cancellation, quotas, 401 integration,
  configured transport bounds and final protocol-drift mapping remain to verify.
  Step 7 is not complete; subsequent review and regression results follow below.
- Resource-401 cache invalidation is now wired through the registry resolver.
  Authorized feedback clears matching completed entries and fences pending fills;
  both new demand and manual refresh avoid invalidated fills. Late completion
  cannot overwrite a replacement, while original waiters can finish. Tests force
  the replacement to complete before releasing the old response. **16 resolver
  tests passed**, Oracle found no blockers, and the dedicated cache-feedback
  bug-finder run returned clean. OAuth generation and auth-context isolation remain
  intact. Full agent-to-provider 401 acceptance is still outstanding.
- Executor transport now has configurable bounds, an executor-wide semaphore and
  a shared single-attempt HTTP client; bridge integration uses these resources
  only for live attempts. Initial transport/config tests passed (**3 executor,
  6 sender**). Config generation ran out of disk; obsolete build binaries were
  removed. Combined verification and regeneration are pending, not waived.
- Protocol `-32602` now triggers a separate durable `ReadRemote` presence lookup
  after the response commit. It forces a charged, fully paginated refresh of the
  admitted source; observed absence maps to `InvalidToolName`, while presence or
  unavailable evidence preserves `InvalidInput`. Projection exclusions count as
  present upstream definitions. No message heuristic is used; MCP `isError`
  remains a custom tool error. The forced refresh is an additional HTTP cost on
  each such protocol rejection, not an uncharged cache check.
- The presence regression passed with removal versus exclusion, quota suspension
  after the remote End but before the presence End, executor reconstruction, and
  completed offline replay. Log: `/tmp/gol36-presence-acceptance.log` (**1 passed**).
- The current MCP 2026-07-28 tools specification permits any JSON value for
  `structuredContent`. Declared strings, arrays and null remain typed; undeclared
  nonobjects retain the JSON-string convention. The import suite passed **84
  tests** after removing object-only restrictions. This corrects an obsolete
  protocol assumption, not the structured-versus-stdout user decision.
- Initial step-7 bug-finder run returned clean, but the parallel Oracle review
  identified missing entity-context idempotency seeds. A stronger retained test
  proved it: lose one response, then restart the executor with the retried request
  held after the upstream effect and before End. Recovery produced **two effects
  instead of one** (`/tmp/gol36-crash-key-before.log`). The clean bug-finder result
  is not sufficient evidence for completion.
- The shared entity boundary now derives a child seed using the caller's normal
  physical/atomic logical position on live and replay, records the admitted
  idempotence mode, and installs that context for native and guest bodies. Entities
  admitted inside caller atomic regions use a Store-local logical child counter;
  mutable atomic leases are not copied across Stores. Wire/payload regressions
  pass **23 tests** (`/tmp/gol36-entity-wire-tests.log`). The stronger lost-response
  and executor-reconstruction regression now observes one effect across retries.
- Shared entity-context verification passed **3 crash tests**: in-process retry
  plus actual executor drop/restart; atomic rollback with a fresh physical Start
  and the same key; and non-idempotent no-resend. The third test nests a guest
  entity in a completed atomic admission, replays a completed generated-key call,
  and repairs unfinished outgoing HTTP with one upstream effect. It exposed the
  need for `generate_idempotency_key` to reserve its logical position before its
  live-only closure, including completed replay. Oracle confirmed the correction
  with no blockers; dedicated entity-idempotency bug-finder run 1 returned clean.
  Log: `/tmp/gol36-entity-review-tests.log`.
- Stdout reconstruction passes with a numbered multi-chunk payload, binary,
  mixed content, empty text and custom error; a committed remote response survives
  executor reconstruction before stdout consumption without another upstream call.
  Log: `/tmp/gol36-policy-stdout.log` (**2 passed**, including policy coverage).
  Cancellation acceptance is being added. Initial runs exposed a stale copied
  fixture and an incorrect test expectation: ordinary cancelled stdout settles
  with `ByteStreamFailure::Cancelled`, not clean EOF.
  Completed reconstruction then exposed attachment cancellation selected after
  aborting the producer, which could choose `Abandoned` first. The shared entity
  coordinator now selects operation cancellation before abort; the MCP body also
  selects cancelled stdout from a recorded cancelled call or presence observation.
  Presence cancellation is no longer swallowed into unavailable evidence.
- The rebuilt guest makes its observed stdout terminal part of a subsequent
  rejected tool-command identity, so a changed replay observation fails a durable
  claim rather than hiding behind a recorded invocation result. The expanded
  regression passes for a pending MCP call, pending presence refresh, and
  backpressured output after the remote End, followed by actual executor teardown
  and offline reconstruction. Log: `/tmp/gol36-cancel-three-windows.log`.
  Oracle found no blockers in the bounded correction, but noted a possible broader
  body-wins terminal race when the nested call succeeded; the dedicated
  `gol36-cancellation-replay` bug-finder run investigated and returned clean.
  This is evidence for the exercised cancellation windows, not proof of every
  ordinary-tool attachment interleaving. Updated walkthrough rendering is pending.
  Step 7 remains open for combined quotas, transport/auth acceptance and review.

### Step 8 — dependency integration pending

- The dependency status changed during implementation: GOL-39 is merged in
  [PR 3842](https://github.com/golemcloud/golem/pull/3842) and is included in the
  latest-main merge for this draft PR. GOL-439 is implementing the chain dispatcher in the
  [middleware thread](https://ampcode.com/threads/T-01a0a94d-ba14-716c-9f45-1f7766424d43).
  Coordinate and integrate that baseline rather than build another dispatcher.
- Its recorded root-chain plan/descendant-position model, typed installation
  parameters, and WIT `streams`/`underlying` split must be reconciled with MCP's
  dynamic leaf activation and the shared entity seed fix. A coordination message
  was sent with the crash reproducer and ownership boundaries. The dependency is
  unfinished; neither its plan nor SDK scaffolding closes MCP middleware acceptance.
- The validated entity seed/scope/proto correction and a standalone guest
  regression were packaged as focused patches against this orb's exact local
  baseline and offered to the middleware thread. No pushes or old whole-file
  replacements. Both patches application-check against that baseline. The other
  thread must adapt its required plan/operation/principal fields and preserve
  generic Host dispatch. Host-side descendants admitted in a different Store or
  later invocation must derive context from recorded root/descendant state, not
  unrelated ambient caller state.
- The middleware thread reports it has adapted the transfer with required
  operation/principal/root-plan fields retained. Common entity and entity/claim
  checks pass there, as does the standalone generated-key crash regression with
  fresh fixtures. Descendants use their parent entity Store's recorded context;
  generic Host leaf activation remains intact.
- The latest middleware-thread report supersedes the earlier fail-closed status:
  a real universal → monomorphic → parameterized → component-leaf chain passes
  all three outer modes there, as do overlapping results after correcting test
  history assertions. This is reported evidence from that checkout, not local
  verification or MCP Host-leaf acceptance. No integration-ready transfer has
  arrived; step 8 remains incomplete.
- Its Oracle review identified a further concurrent idempotency issue:
  `from_started_request` reserves the logical position after asynchronous
  Start/claim resolution, whose completion order can differ on replay. That
  thread owns capturing the caller key, logical position and idempotence mode
  synchronously with the tool attempt ordinal, carrying the capture through
  admission, and adding reordered-claim/atomic coverage. Do not duplicate that
  correction here; integrate it with the eventual dispatcher transfer.
- The subsequent GOL439 report confirms fresh passing idempotency, real typed
  middleware-chain dispatch and overlapping-replay tests in its unpushed local
  `main`, not `origin/main`. Generic Host dispatch remains, but it has not imported
  MCP activation implementation. Its remaining contract question is typed
  schema-value stream readiness versus full settlement of admitted children;
  that question is recorded in its Linear plan for user clarification.
  It reports TypeScript **835 passed, 20 skipped**, Effect **864 passed**, refreshed
  runtime templates, and fixes for borrowed-observer disposal and Scala admission
  stdout. The broader entity suite is **10/11**; the remaining white-box fixture
  lacks a required live-admission operation and is being repaired there without
  weakening the invariant. No ready transfer or push exists, and these reports
  do not establish completion of GOL439 steps 7/9/10 or GOL-36 acceptance.
- The latest dependency read confirms the stream-settlement decision is resolved:
  reuse agent RPC semantics, exposing available stream-bearing results early while
  keeping producers/children alive until settlement. No new public completion API.
  A real typed-output-through-middleware test still hangs before the caller's
  first item; that thread owns debugging it and its remaining crash matrix.
  There is still no integration-ready patch/bundle. Its new async Rust tool-start
  API also requires adaptation of generated consumers during eventual integration.

### Step 9 — completed

- Periodic refresh now targets active successful cached views, including
  historical deployments, using their authorized context and the shared refresh
  path. The default interval is one minute; existing cache/fetch bounds apply.
  The loop is owned by the registry task set and retains only a weak service
  reference between refresh rounds. Failed upstream refresh retains prior usable
  metadata; authorization failures evict the view. Focused replacement/removal/
  failure and config checks passed **2 tests** before the final interval-range
  validation addition. Full resolver review, bug-finder and generated-config
  comparison remain pending. Disk-full linking was recovered by removing only
  disposable build artifacts.
- Periodic work now uses bounded background concurrency and only refreshes views
  used within the cache TTL; background work never renews that demand window.
  OAuth rotation uses the actual resolved credential identity, preserving the
  current generation when an older failed sibling exists. Foreground demand
  timestamps survive a background replacement. Oracle follow-up found no blockers.
- Periodic-refresh bug-finder run 1 found queued work could resurrect an evicted
  historical view. The original cache key now crosses context resolution and is
  checked under the insertion mutex: the view must still be valid, successful and
  recently used. Run 2 confirmed resolution, then found saturation blocked an
  invalidated pending entry's same-key replacement. Saturation now rejects only
  new-key insertion; superseded fills remain fenced by channel identity.
  All **25 resolver tests passed** (`/tmp/gol36-periodic-capacity.log`), and Oracle
  confirmed both corrections without blockers.
- Run 3 confirmed the saturation fix and found that `operation_timeout =
  Duration::MAX` passes `McpImportResolverConfig::validate` despite overflowing
  deadline construction. Its retained failing regression is
  `config::tests::mcp_import_resolver_rejects_unrepresentable_operation_timeout`.
  Fingerprint: `mcp-import-operation-timeout-unrepresentable`.
  Three successive runs with new findings triggered the mandatory design
  checkpoint; implementation paused without overriding it.
- Checkpoint assessment: the first two findings share cache-admission semantics
  (obsolete work is not demand; replacing a key does not consume another slot).
  Those invariants now live at mutex-protected insertion. The remaining finding
  is a separate validation gap, not evidence that the cache needs another layer.
  Proposed correction: validate every resolver duration used to construct an
  absolute deadline at its owning configuration boundary, rejecting values that
  cannot be represented; retain elapsed-time-only TTL semantics and defaults.
  Exercise rejection through startup validation and retain the overflow
  regression. This boundary review and a further bounded run require explicit
  user approval. Generated-config comparison remains outstanding.
- The user approved the validation correction. `operation_timeout` now rejects
  zero and values for which `Instant::checked_add` fails, matching the existing
  refresh-interval check. Shared transport already checks its own timeout;
  elapsed-time-only TTLs deliberately still accept `Duration::MAX`. The retained
  regression also verifies startup fails before DB creation or background tasks.
  **27 targeted tests passed** (`/tmp/gol36-deadline-validation.log`), and Oracle
  found no blockers. Defaults and serialization are unchanged by this correction.
- Authorized bug-finder run 4 found a remaining time-of-check/time-of-use edge:
  a duration one second below the clock's maximum representable deadline passes
  validation, then overflows two seconds later. This is approximately 292 billion
  years on this platform, not an ordinary timeout. It reused fingerprint
  `mcp-import-operation-timeout-unrepresentable`; the design checkpoint remains
  active. The provisional test
  `mcp_import_validation_does_not_accept_a_timeout_that_soon_becomes_unrepresentable`
  remains failing; it will need to expect rejection under a fixed-limit fix,
  rather than unconditionally unwrapping validation success.
  Proposed resolution: impose a fixed one-day supported ceiling on resolver
  operation timeouts and refresh intervals, retaining defaults and TTL semantics.
  This enforces a practical supported range instead of a time-dependent maximum.
  The user approved this limit. Both durations now accept only positive values
  at most 86,400 seconds. Tests cover the exact limit, one nanosecond above it,
  transport/operation ordering and startup rejection before database creation.
  **28 targeted tests passed** (`/tmp/gol36-deadline-cap.log`); Oracle approved.
  Bug-finder run 5 resolved the timeout finding with no new findings, no design
  checkpoint and no non-convergence signal. This bounded review is closed.
- Deployment-pinned public inspection (`GET .../mcp-imports/:index/tools`) and
  explicit refresh (`POST .../mcp-imports/:index/refresh`) now share the resolver.
  Responses expose public Tool definitions, upstream names, projection digests
  and per-import diagnostics, not credentials or internal mapping trees.
  Environment/deployment visibility is required (otherwise 404), followed by
  ViewTools (otherwise 403). Refresh uses these same permissions and the owner's
  HTTP quota; failures are explicit, whereas ordinary reads can retain usable
  stale metadata. Operator and runtime cache contexts remain separate. Named
  MCP import error codes distinguish upstream rejection/unavailability and
  projection failure from internal errors.
  **49 MCP/OAuth/route/error tests passed** serially. Oracle follow-up found no
  design blockers. Bug-finder caught scheme resolution preceding visibility;
  authorization now precedes import/scheme resolution. Run 2 confirmed that
  regression resolved with no new findings and **6 current targeted tests passed**.
  This bounded review is closed; generated OpenAPI/client/docs work follows.
- Codegen bootstrap investigation confirmed there is no staged import declaration:
  build precedes deployment, and the deployment plan only contains current imports.
  Using that revision would break first build and silently use old declarations
  after edits. Oracle recommends environment-scoped declaration resolution before
  build, with deployment-write and tool-view permissions, shared owner credential/
  quota/projection machinery, and no deployment mutation or preview cache.
  OAuth consent must also be possible for a declared target before first deploy;
  grants already bind environment/scheme revision/owner/resource independently of
  deployment. Existing deployed-target operations and declared-target operations
  serve distinct lifecycles, not old/new protocol compatibility, and must share
  the same credential and consent implementation.
  Imports not consumed by codegen may still deploy without consent. Generating
  typed clients for protected imports requires consent first; unavailable metadata
  fails that build rather than silently generating stale or partial definitions.
- Declaration resolution and declared OAuth routes are implemented with shared
  credential refresh, owner quota and projection. The sanitized consent session
  binds the exact declared/deployed target; the grant remains independent of a
  deployment. Tests exercise consent/refresh before any deployment and subsequent
  deployed reuse, changed-target rejection, preview pagination/precedence/cache
  isolation, route shapes, individual permission requirements and 401 invalidation.
  Preview currently resolves sequentially under one deadline, with at most 128
  declarations, request limits and aggregate listing budgets. Consent retains
  the specified security-scheme Update permission; preview additionally requires
  View, Deploy and ViewTools. Oracle found no blockers; its indexed HTTP error
  and missing boundary-test findings were fixed. The combined MCP/OAuth suite
  passed **64 tests**. Bug-finder found validation after native-name deduplication;
  validation now runs on the original list. Count, byte and valid-duplicate
  regressions passed with **10 focused tests**. Run 2 resolved the finding with
  no new or recurring findings; this bounded review is closed. OpenAPI/client/docs
  regeneration and the CLI codegen consumer follow; this does not complete step 9.
- CLI declaration-based codegen now feeds the ordinary bridge planner and all five
  generators. Bare imported dependencies are resolved before consumer builds;
  explicit native/release declarations reserve names. Freshness uses the declaring
  manifest file plus import index/projection digest, never the application tree.
  **32 targeted CLI tests passed** (`/tmp/gol36-codegen-tests.log`). Oracle's
  freshness and test-compilation blockers were fixed; follow-up found no blockers.
  The dedicated `gol36-cli-codegen` bug-finder run returned clean.
- CLI declared consent adds `--manifest` to authorize/complete/status/disconnect,
  mutually exclusive with `--revision`. Exact selected-environment declarations
  use the shared environment-variable renderer and declared OAuth routes without
  requiring deployment. Structured OAuth outputs use a null deployment revision
  for manifest targets. **34 MCP/output-schema tests passed**
  (`/tmp/gol36-cli-consent.log`). Oracle verified target/secret/schema contracts
  and found missing help text; the correction and **38 MCP/command tests** passed
  (`/tmp/gol36-cli-consent-docs.log`). Oracle follow-up found no blockers and
  `gol36-cli-consent` bug-finder run 1 returned clean.
- Projected-client acceptance now compiles actual consumers in all five languages.
  The tests exposed bare binary fields in mixed content, now represented using
  the canonical SDK unstructured-binary carrier, an unused MoonBit import, and
  Effect wrapper narrowing in the shared TypeScript encoder. **29 projection
  tests** and **9 compiler/adjacent regressions** passed. Oracle's Scala shared
  build-directory concurrency finding was fixed by moving that test into the
  existing sequential suite. The final **5/5 consumer rerun** passed
  (`/tmp/gol36-mcp-consumer-final.log`); Oracle follow-up approved the slice and
  `gol36-projected-consumers` bug-finder run 1 returned clean.
- Deployment discovery now produces ordinary `McpImportDiscovery` validation
  warnings through the shared declaration preview, preserving operator permissions
  and environment-owner billing. Missing consent or unavailable providers do not
  prevent storing valid declarations. Native/earlier-import collisions carry
  index/name diagnostics. **58 resolver/OAuth/API/deployment tests passed**
  (`/tmp/gol36-deployment-warnings-final.log`). Oracle's assertion correction was
  applied; `gol36-deployment-warnings` bug-finder run 1 returned clean. Preview
  remains all-or-nothing under one 25-second deadline: a later failure can replace
  earlier diagnostics with the import failure, but never publish partial tools.
- The operator guide explains predeployment consent, provider network permission,
  owner credentials/billing, refresh/revocation, typed clients and structured versus
  stdout output. Prettier, docs-version and link checks passed. Rendered content
  was inspected; a clipped code comment was fixed and reinspected. Representative
  screenshot: `.amp/in/artifacts/gol36-mcp-import-docs.png`.
- OpenAPI, generated clients and REST environment docs are regenerated. Structured
  CLI schema now models deployment warnings, including nullable index/name and
  the u32 index bound. **17 schema tests and 3 extra generated-example property
  runs passed**, as did `check-cli-output-schema` and schema summary generation
  (`/tmp/gol36-output-schema-tests.log`, `/tmp/gol36-output-schema-check-task.log`,
  `/tmp/gol36-output-summary-task.log`). Fresh registry TOML/env dumps exactly
  match both checked-in configuration references, including MCP/OAuth defaults.
- A real CLI-to-Golem-to-provider acceptance test generates Rust clients before
  initial deployment, exercises bearer/basic runtime credentials, asymmetric
  structured output, simple stdout/MIME and ordered typed mixed content without
  stdout. Four calls carry four distinct keys. Provider shutdown and a Golem
  restart reconstruct prior agent state offline. Final strengthened run passed
  in **125.7 seconds** (`/tmp/gol36-mcp-e2e-final.log`). Oracle found no blockers;
  its fail-fast request/MIME/ordering/warning suggestions were applied, and
  `gol36-cli-e2e` bug-finder run 1 returned clean for acceptance/schema changes.
  This closes step 9, not OAuth combined acceptance or middleware integration.

### Step 10 — combined acceptance in progress

- The real CLI OAuth acceptance test uses predeployment manifest consent, an HTTPS
  provider, generated Rust clients and the registry credential RPC. It independently
  checks PKCE, client authentication, resource/redirect parameters and discovery.
  A successful call is followed by a resource 401 without automatic resend; a
  later call refreshes once with the rotated token. Provider shutdown and Golem
  reconstruction preserve the success/error/success history offline.
  The targeted run passed in **115.2 seconds** (`/tmp/gol36-oauth-final-e2e.log`).
  Oracle's background-refresh race finding was then fixed by capturing token
  counts at the rejected request rather than after unrelated background work.
  The corrected test passed the `gol36-cli-oauth-e2e` bug-finder baseline; run 1
  returned clean. The test trusts the fixture CA explicitly, without disabling TLS
  validation. This closes the combined OAuth acceptance case, not step 10.
- Combined executor testing exposed a public-oplog protobuf depth failure for
  discovery metadata. `SerializableDiscoveredTools` preserves structured binary
  payloads but renders complete JSON text in the public schema; MCP call Starts
  render their already-typed input directly. Three focused payload/public-oplog
  tests pass, including deep real projected metadata and protobuf/WIT conversion
  (`/tmp/gol36-discovery-public-oplog-tests.log`). Oracle found no blockers.
- The first combined rerun passed **12 of 13 tests**. Its new quota test revealed
  that the test worker constructor ignored per-invocation limits even with the
  new resource-limit override. The constructor now reads the same account limits
  as production; all **13 combined executor tests passed**, including unchanged
  zero/one/monthly-exhausted assertions (`/tmp/gol36-quota-combined-corrected.log`).
  The bug-finder's proposed error-category conflict
  was a mistaken diagnosis: the actual observed tool result was successful, not
  a tool error. Oracle approved the correction and `gol36-oplog-quota-acceptance`
  bug-finder run 2 returned clean, closing this slice.
- Final Step 4/7 Oracle review approved the bridge subject to the now-passing quota
  test. It requested live revoked-grant recovery acceptance, deployment-time
  rejection of unsupported protocol versions, and missing executor/debug-service
  config references. All three corrections are now implemented and verified.
  The extended OAuth test passes in **156.1 seconds**, with three successful calls,
  three token exchanges and one rejected upstream request; disconnect prevents
  dispatch/exchange, fresh consent restores access, and the full history replays
  offline (`/tmp/gol36-oauth-revoked-fresh-e2e.log`).
- Protocol constants now have one source of truth shared by deployment validation
  and runtime transport. Unsupported overrides fail deployment instead of becoming
  discovery warnings. All **85 import tests** and **29 common-model/entity/diff
  fingerprint tests** pass; the import crate also builds standalone against the
  non-full common model. The diff fingerprint is unchanged because only test
  fixtures changed in diff modules. Scoped strict library Clippy fixed the two
  nested-if lints; strict **all-target common/import Clippy passed**
  (`/tmp/gol36-common-import-clippy.log`).
- Fresh executor/debugging binaries regenerated their TOML/env references, with
  no default changes beyond the new MCP transport fields. Oracle approved the
  protocol/revocation corrections and `gol36-auth-completion` bug-finder run 1
  returned clean. No additional autonomous review loop is needed for this slice.
  All **3 executor/debug config tests passed** (`/tmp/gol36-config-tests.log`),
  closing the final review conditions for steps 4 and 7. Scoped formatting and
  diff checks pass. Broader executor compilation still reports three existing
  warnings in untouched reconstruction/instance code; repository-wide CI has
  not been run or claimed green.
- The durability walkthrough and skill now explain the metadata representation
  and HTTP budget ordering. Both affected rendered walkthrough sections were
  inspected at 2× scale (`.amp/in/artifacts/gol36-durable-discovery.png` and
  `.amp/in/artifacts/gol36-durable-mcp-execution.png`).
- Middleware dependency status: the RPC-style early-result contract is decided
  and the promise-gated typed-stream restart test passes. GOL-439 also found an
  overlapping entity atomic rollback could erase sibling history; its scoped
  rollback correction and explicit atomic-overlap/crash acceptance remain with
  that thread. No integration-ready patch/bundle is available yet; no remote
  branch contains that local unpushed work. Step 8 remains required, and its
  dispatcher/rollback work must not be duplicated in the MCP bridge.

### Draft PR and latest-main integration

- [Draft PR 3907](https://github.com/golemcloud/golem/pull/3907) retains steps 8
  and 10 as deferred until GOL-439 is reviewed and merged. It is not ready for
  final acceptance or merge.
- The main merge preserves middleware metadata alongside MCP imports, assigns
  MCP imports protobuf field 6 after middleware fields 4/5, and moves the MCP
  migrations to 039/040 after main's 036–038. The combined diff model is version
  10; main's historical fingerprints are preserved.
- Oracle caught lost MCP-only deployment rendering in the CLI's renamed module;
  it is restored. MCP custom errors now retain the declared `mcp-tool-error`
  name required by main's native-output validation. The focused merge bug-finder
  run returned clean. Common-model tests passed 23/23, and the new fingerprint
  passes with golden-file updates disabled. Broader merge/CI validation is ongoing.
- The first CI run exposed missing MCP/middleware fields in test constructors
  plus two Clippy errors. Constructors now supply the intended empty/default
  values. The existing `CompiledTools` value crosses the repository conversion
  intact, and the resolver's equivalent nested condition was auto-fixed.
  Oracle found no blockers and the focused CI-fix bug-finder run returned clean.
  Local validation passed 30 CLI, 38 common, 27 executor, 1 environment roundtrip,
  85 MCP, 44 registry MCP/OAuth, and 2 SQLite snapshot/accounting tests (227 total),
  plus registry library Clippy and scoped formatting. CI rerun remains pending.
- Subsequent CI passes exposed test-only lint and merge repairs: an incomplete
  entity replay test now supplies the same seed and modes as its live scope,
  and the middleware fail-closed test checks the coherent owner-component
  deployment lookup instead of the replaced activation API. Both repairs passed
  focused Oracle and bug-finder reviews. The replay-authorization test, all 12
  discovery integration tests, and MCP stdout integration passed locally after
  rebuilding fixtures against the merged WIT. Executor all-target Clippy passed,
  as did 23 environment-state tests, the generated-key/incomplete-HTTP replay
  regression, and the TypeScript bridge compile check after correcting its stale
  source assertion.
- CI's full build, unit/generated checks, and 49 jobs passed. Remaining failures
  identified a snapshot-test metadata race, a fixture-size-dependent memory
  assertion, the already-corrected lookup assertion and test lint, and a genuine
  nested-output mapping race. Separate session runtimes now refresh their mapping
  tables under the shared lock before allocating nested transport IDs. Internal
  mapping recovery uses the authoritative epoch after resume; transport-bound
  runtimes remain fenced to their accepted epoch. Oracle approved this correction
  and bug-finder resolved its epoch finding with no new findings. All 62 durable
  session unit tests and 4 metadata/snapshot/nested-stream integration tests pass.
  The nested-stream restart regression also passed 20 consecutive targeted runs
  (the unfixed version failed 3 of 11). The next CI run remains pending.
- Follow-up observations outside this CI fix: other unbound foreign-stream
  attachment/reader paths still use their resident default epoch, and generic
  mid-stream protocol-failure terminalization needs separate investigation.
  These are not claims that the deferred GOL-439 integration is complete.
- Registered tool chains remain fail-closed until middleware runtime integration.
  Dynamically discovered MCP tools do not yet receive universal middleware chains;
  step 8 must explicitly construct their dynamic Host leaf plans through GOL-439.
  The provisional whole-envelope transport bounds and separate projection budgets
  remain a final-review item, as previously agreed.

## Review and decision history

- Oracle reviewed the initial plan, then conditionally approved a corrected plan.
  Accepted findings include missing middleware dependencies, fixed native catalog
  constraints, full dynamic snapshots, transport retry controls, and the
  distinction between upstream-offline replay and registry rehydration.
- Rejected scope reductions: dropping OAuth, dropping middleware acceptance,
  ignoring caller-context credential resolution, and changing import collision
  precedence. A minimal SDK adapter is preferred over prematurely committing to
  a hand-written MCP transport.
- Subsequent user decisions supersede review recommendations: no standalone
  idempotency refactor; lifecycle/deployment behavior follows existing tools;
  authentication follows the original spec, not a newly invented identity model.
- User agreed to retaining usable last-successful metadata on refresh failure,
  explicit manual-refresh errors, authoritative successful refreshes, and rejecting
  affected new calls when refreshed metadata is incompatible with middleware.
- User agreed to excluding only tools with invalid or unrepresentable definitions,
  keeping valid tools available, reporting precise reasons, and retiring old live
  definitions when a successful fetch makes them unrepresentable.
- User prioritized generated typed clients and structured values for mixed output,
  then explicitly clarified that simple strings/binary payloads still use stdout.
  The blanket structured-output rule was an overgeneralization and is withdrawn;
  the original example's stdout behavior is retained, but its no-result metadata
  shape is replaced by the fixed result record.
- Final Oracle review: **approve with amendments**. Incorporated concrete error
  mapping/rendering, text MIME type, optional stdout declaration and omitted-output
  consequences, commit-before-output ordering, progress handling, cold-cache
  failure semantics, deploy-before-consent, credential refresh ownership, dynamic
  middleware validation, and dependency/ordering clarifications.
- Deployment review finding accepted and resolved without reopening user policy:
  permanent owner pinning would change existing config-only redeploy behavior.
  Replace it with existing live selection plus an exact deployment reference per
  durable observation/admission. A focused Oracle follow-up verified the existing
  activation reference and immutable deployment storage and judged this approach
  sound, requiring no permanent pin or user confirmation. The plan now includes
  coherent snapshot retrieval, exact-revision RPC/cache, missing-revision errors,
  and targeted replay tests.
- Review suggestions are not new requirements: no blanket per-native metadata
  digest list in the oplog; retain existing integrity validation and add fields
  only when necessary. No invented import-level middleware override or automatic
  credential-owner substitution. Output layout, error rendering, and OAuth CLI
  workflow details remain explicit engineering choices implementing the contract.
- Planning-stage observations and reviews were not execution evidence. Subsequent
  implementation verification is recorded separately above; unimplemented steps
  and untested suspected pre-existing bugs remain unproven.
