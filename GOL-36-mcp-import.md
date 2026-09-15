# GOL-36: MCP import — work-in-progress specification and plan

Status: implementation in progress. Steps 1–2 completed after tests, Oracle review,
and the bug-finder loop; step 3 is in progress. Middleware remains an implementation
dependency. The finalized planning snapshot is attached to GOL-36 in Linear.

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
- [ ] **3. Implement shared projection/conversion.** Cover names, filters,
  precedence, exact JSON field mappings, schemas, documentation, annotations,
  errors, typed upstream results, mixed-content variants, and simple text/binary
  stdout. Specify unsupported cases and bounds explicitly; test projection
  independently of network behavior.
- [ ] **4. Implement policy-aware MCP transport and authentication.** Inspect SDK
  customization before choosing an adapter. Support the selected full Streamable
  HTTP contract, per-call keys, cancellation, and network accounting without
  hidden retries. Implement bearer/basic and outbound OAuth as separately
  estimable work units, both in scope. Follow the OAuth workflow and protocol
  boundaries above. Add an in-process mock upstream and provider fixture; the
  bearer/basic path can unblock resolver/bridge work while OAuth is implemented.
- [ ] **5. Implement registry resolution on demand.** Share resolution across
  listing, lookup, invocation preparation, cache warming, and refresh. Handle
  pagination, auth-scoped caching, coalesced misses, successful empty lists,
  failures, precedence, and diagnostics. Do not rely on warm caches.
- [ ] **6. Separate fixed and dynamic discovery persistence.** Rehydrate native
  definitions from the compact exact-deployment reference recorded for the
  observation/admission. Record dynamic MCP observations, rejections, and
  invocation snapshots with full metadata required by replay. Reuse existing
  activation references; retain non-derivable state. Preserve normal durable-call
  sequencing and infrastructure retry behavior, without a new owner association.
- [ ] **7. Connect the special native MCP bridge.** Reuse native execution,
  authorization, accounting, cancellation, and existing HTTP/RPC idempotency
  techniques. Persist/replay the nested remote result, including structured content
  and simple-output stdout, without repeating completed upstream effects.
- [ ] **8. Complete middleware integration.** On the middleware dependency
  baseline, apply universal and per-tool chains, supply recorded layer-appropriate
  metadata, and dispatch through runtime-minted underlying handles. Revalidate
  on refresh and surface incompatible drift without bypassing middleware.
- [ ] **9. Add operator and codegen surfaces.** Manual refresh, periodic host
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
