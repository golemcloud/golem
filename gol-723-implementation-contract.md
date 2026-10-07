# Explicit phantom selection on HTTP mounts

## Review delivery status

Steps 1–8 have explicit oracle approval. Step 9 remains blocked, not conditionally
approved: integrated tests pass with locally rebuilt, checksum-verified built-in
tool overrides, but the published default artifacts still expose the old agent
metadata ABI. Their replacement, updated pins, and default-startup verification
remain outstanding. No compatibility workaround is included.

The owner has authorized opening a review PR before publishing the replacement
tools, with the expected CI startup failures disclosed. This supersedes the
planning-stage restriction on pushing a review branch and opening a PR below;
publication, merge, deployment, and release remain unauthorized. The planning
sections retain their original decisions and acceptance cases.

## Step 1: behavior and scope

Implementation baseline: `origin/main`, revision
`4c49e77729fd2c2a1a285b6dbce1491543dfdd51`.
The implementation follows Linear GOL-723 comment
`f47719ef-c924-412a-900b-35f3cf3fe3ec`; every step requires explicit oracle approval
before the next step begins. This document records decisions, not implemented behavior.

### Selection contract

| Mount without an explicit selector | Identity policy to preserve |
| --- | --- |
| Durable REST, `phantomAgent=false` | Original constructor-derived instance |
| Durable REST, `phantomAgent=true` | Fresh random phantom per request |
| Ephemeral REST | Invocation-derived identity |
| Durable Streams, durable agent, flag false | Original root |
| Durable Streams, durable agent, flag true | Session-derived phantom root |
| Durable Streams, ephemeral agent | Invocation-derived identity using session key |

A mount may declare exactly one phantom selector: an ordinary path capture or a
named query parameter. Requiredness defaults to required. A supplied UUID selects
that phantom of the component/constructor-derived base agent, never a global UUID
lookup. An omitted optional selector explicitly selects the original instance,
including when `phantomAgent=true`. An unused UUID follows normal agent creation;
it does not clone existing state. Stream forks remain derived from the resolved
root and fork name.

Missing required query values, empty or malformed UUIDs, and repeated selector
query values (including identical values) return 400. A missing required path
segment does not match the route. UUID parsing failure never retries the absent
optional route. UUID spelling is canonicalized in generated selector-bearing
stream links. UUIDs address agents; they do not grant authorization.
Existing authentication runs before selector resolution and identity-dependent
cache lookup; existing authorization and principal propagation remain intact.

### Initial restrictions and rationale

- Explicit selectors are rejected on ephemeral agents and HTTP routers. Their
  invocation/routing identity policies are not existing durable-phantom selection.
- Explicit selectors are rejected on mounts exposing agent filesystem bindings.
  Method and filesystem dispatch currently select identities separately; allowing
  a selector only on methods would expose the original agent's files while invoking
  a phantom. Selected-phantom filesystem access is not part of this change.
- Signed webhook callback addressing remains independent and unchanged. Callbacks
  from selected phantoms must still target the correct signed agent identity.
- Durable Streams selectors are supported, not silently omitted from scope.
  Selector names `offset`, `live`, and `cursor` are rejected on these mounts.
- There are no compatibility shims or legacy protocol paths.

### Optional paths and collisions

An optional path selector removes its entire segment. Expand the mount into
present and absent concrete alternatives before binding constructor arguments.
Recompute capture ordinals (variables, not literal segments), method ordinals,
stream base capture counts, route IDs/families, and OpenAPI parameters for each.
The absent alternative carries explicit original selection, not existing policy.
Webhook callback routes are added independently, once.

A path selector references exactly one ordinary mount capture, not a system
variable or catch-all, and cannot also bind constructor or application inputs.
Query collision checks use external parameter names. Reject new ambiguity from
optional expansion: two concrete alternatives with overlapping matching languages
and different target/binding semantics, including catch-all overlap. Preserve
existing precedence for unrelated routes. Literal precedence must not hide an
invalid selector by routing the same selector-bearing request to the omitted
alternative. Concrete collision acceptance cases are reviewed in step 2.

### Durable Streams resource URL decision

Generated session/fork locations and lazy-creation guidance retain the resolved
selector, insert resource suffixes before queries, and canonicalize UUID values.
Retain existing origin-relative links and existing omission of ordinary method
queries/read controls; this change adds only the selector to generated links.
Invocation creation and reattachment still supply their method inputs as today.
There is no change to existing-stream PUT configuration/argument checks.

Resolve the selected root first. Derive every named fork from that exact root and
the public fork name, including sources that are themselves named forks. Keep
`Stream-Forked-From` path-only and target-root-scoped as today. Its source resolves
inside the target's selected root; the header cannot select a different root.
Retain existing source route/session/slot restrictions, canonical source-path
persistence and replay comparisons, and executor fork/export machinery.

For query selectors, clients use the path component of a source stream link in
`Stream-Forked-From`, and include the root selector in the target request URL.
Do not accept query-bearing source headers, add canonical alias paths, or promise
independently complete or cross-root source references. Session links are not
stream-slot fork sources. This owner-approved scope supersedes the saved plan's
stronger independent-source URL requirements.

### SDK and generated-contract commitments

TypeScript and Effect expose `Http.phantomId.path/query(name, { optional: true })`
in mount options; omitted options mean required. Rust exposes
`phantom_id(path/query = "instance", optional = true)`. Scala exposes
`phantomIdPath` or `phantomIdQuery` and `phantomIdOptional`. MoonBit exposes
`#derive.mount_phantom_id(path/query="instance", optional=true)`. Both sources
together are invalid. The selector is not a constructor/method input. Retain
constructor coverage validation and TypeScript literal inference, exempting only
the declared selector.

Update authoritative WIT, domain metadata, metadata protobuf, both WIT conversion
directions, compiled-route types and their separate protobuf transport, all SDK
producers (including Effect and shared router metadata), generated bridges,
custom deployment OpenAPI, management OpenAPI/client/docs, and CLI projections.
Use repository generation tasks rather than hand-editing generated files.
Only change deployment diff model versioning if its representation or hashing
actually changes.

### Verification obligations

Acceptance cases are frozen in step 2 before implementation. Layer tests accompany
their owning changes and run before approval. Integrated verification includes
original and genuinely forked divergent state, repeated path/query requests,
omission under both flag values, asymmetric captures around optional segments,
invalid/duplicate selectors, all unbound modes, stream URL and retry round-trips,
same-session distinct-root isolation, auth/ETag isolation, and selected-phantom
webhook callbacks. Rebuild selected fixtures, run component-size analysis when
generation/bridges change, check generated drift, perform bounded bug finding,
and obtain final oracle approval. No push, PR, merge, or release is authorized.

### Step-one review outcome

Initial oracle reviews blocked advancement pending the stream URL product
decision. The owner subsequently approved preserving existing root-scoped fork
semantics, path-only source headers, selector-bearing returned links, and no
canonical aliases or independently addressable cross-root sources. Step 1 is
approved by oracle with that decision. No runtime or test changes were required
for that planning gate. Constructor-query bindings do not exist at this baseline;
constructor path bindings stay unchanged.

## Step 2: acceptance cases and test ownership

Tests are implemented alongside their owning layers, not as non-compiling tests
against missing APIs. This acceptance specification precedes implementation.
Let P = `550e8400-e29b-41d4-a716-446655440000` and
Q = `8badf00d-1234-4567-89ab-0123456789ab`. Use distinct constructor values `d7`
and `d9`, method arguments `left=17` and `right=29`, and divergent original/fork
state (original 11, genuinely copied fork mutated to 37).

### Shared metadata and transport (step 3)

Owners: `golem-common/src/schema/agent/http/tests.rs`,
`golem-common/src/schema/agent/wit.rs`, schema protobuf tests, and
`golem-service-base/src/custom_api/protobuf.rs` codec tests.

- Round-trip required/optional path and query declarations through both WIT and
  metadata protobuf directions. Retain name, source, optionality, and flag values.
- Round-trip compiled existing-policy, original, bound-path-index, and bound-query
  selections, including query optionality. Absent optional path compiles to
  explicit original and remains original after transport serialization.
- No declaration retains existing metadata semantics. Required is the default in
  each SDK producer, not an implicit reinterpretation of old serialized data.
- Reject empty selector names, unsupported agent mode/kind/filesystem mount, and
  malformed binding metadata rather than silently accepting an unusable mount.

### Deployment and OpenAPI (step 4)

Owners: registry `http_parameter_conversion.rs`, `route_compilation.rs`, and
`deployment_context/tests.rs`; worker-service OpenAPI tests.

- Required path mount `/decisions/{decision}/{instance}` matches P/Q and binds
  `decision=d7`; omitted path does not match. Required query on
  `/decisions/{decision}` exposes one required UUID parameter `instance`.
- Optional path mount `/decisions/{decision}/{instance}/literal/{tenant}` plus
  endpoint `/compare/{left}/middle/{right}` expands to present/absent alternatives.
  With `decision=d7`, `tenant=west`, `left=17`, `right=29`, present indexes are
  constructor 0/2, selector 1, method 3/4; absent indexes are constructor 0/1,
  method 2/3. Literal segments never increment capture indexes. Compile stream
  families from both alternatives with respective base capture counts 5 and 4.
- Optional path endpoints expose two concrete OpenAPI paths, each with the correct
  constructor/method UUID/string/numeric parameters; only the present path exposes
  the required UUID selector parameter. Optional query is one optional UUID query.
- Reject selector absent from mount, repeated selector capture, selector that is
  also a constructor/method capture, catch-all/system variable selection, and
  query selector conflicting with a method's external query name even when its
  internal field name differs. Retain constructor coverage for all other fields.
- Reject ambiguity introduced by expansion: present `/x/{instance}/tail` and
  absent `/x/tail` with endpoint catch-all can accept the same path; a new omitted
  alternative equal to another same-method endpoint or overlapping its catch-all
  is also rejected. Do not change preexisting unrelated literal-route precedence.
- A variable endpoint `/x/{instance}/{value}` with omitted alternative `/x/{value}`
  is non-overlapping and accepted. P/non-UUID requests with an explicitly occupied
  selector segment never fall back to absent selection. Distinguish GET/POST when
  checking overlap, except method-independent mount-prefix behaviors.
- Reserve stream query names `offset`, `live`, `cursor`; signed webhook routes are
  emitted once, not for each optional alternative. Route IDs remain unique and
  route capacity checks produce no partially compiled stream families.

### Request identity, authentication and caching (step 5)

Owners: worker-service `custom_api/call_agent/mod.rs`, cache-header tests,
real custom-api tests `readonly_http.rs` and principal fixtures.

- Required query: missing, empty, malformed, duplicated identical P/P, and distinct
  P/Q all return 400. URL-decode before parsing names/values; one valid UUID works.
- Optional query omitted selects original under both `phantomAgent` flag values;
  `instance=` is invalid, not omitted. Present optional path/query selects P under
  both flags. Optional path absence selects original under both flags.
- Canonically equivalent accepted UUID spellings resolve to the same AgentId.
  P on constructor d7 and P on constructor d9 identify different agents.
- Real fork test copies state 11 then changes only P to 37; repeated mounted path
  and query reads return 37 for P and 11 for original. Q follows ordinary creation
  rather than cloning P or the original. Preserve constructor/method argument order.
- ETag obtained from original or P cannot return 304 for Q. Invalid/duplicated
  selector with a valid ETag still returns 400, not 304. Missing/invalid auth is
  rejected before selector/cache handling; valid-auth matching-P ETag still 304.
- Regress unbound durable original, durable random per request, ephemeral
  invocation-derived identity, principal propagation, and selected-phantom signed
  webhook callbacks without changing callback addressing.

### Durable Streams (step 6)

Owners: worker-service `durable_streams/{mod,session,fork,read}.rs` and integration
`custom_api/durable_streams.rs`, extending existing transparent/divergent forks and
generated-location fixtures. Executor export/replay is regression-only.

- Path/query required/optional selectors choose root P; optional omission chooses
  original even with the flag true. Unbound durable original/session-derived and
  ephemeral session-key policies remain unchanged.
- Create identical public session `s1` under original, P, Q with distinct appended
  messages. Reads/writes/deletion affect only selected root. Follow returned
  session/fork links without losing the root; generated suffixes precede queries.
  Query links include only the canonical selector, not `offset/live/cursor` or
  creation-only method queries. Optional omission produces no selector query.
- For P target `/forks/alternative/...`, path-only source `/.../streams/events`
  resolves P's stream; source `/forks/first/...` resolves P's `first` fork. Original
  and Q's identically named forks remain unchanged. Fork-of-fork inherits the
  chosen source prefix but target identity derives from P + target fork name.
- Same fork creation/config retry returns success; changed source fork, offset,
  sub-offset or content/config conflicts retain existing responses. Equivalent
  accepted selector spelling does not cause receipt conflicts. Malformed selector
  fails before export; source query/fragment/absolute URL, mismatched method base,
  session, slot or selector-bearing path to another root remains rejected.
- A query-bearing source reference is rejected; clients pass its path component
  and put selection on the target URL. No target may select its source root through
  the header. Query roots P and Q may reuse the same source pathname safely because
  source-agent identity is included in executor receipt matching.
- Selecting a known DS fork UUID directly can read its published session given
  matching component/constructor/method/slot. A further named fork uses that exact
  selected phantom as its root. Do not expect SDK self-fork alone to publish
  inherited sessions or confuse root PUT with DS fork PUT semantics.
- Retain JSON/bytes read, append, close, expiry, tombstone, auth and admission
  behavior on selected roots and regress nested fork prefix divergence.

### SDKs, generated artifacts and integrated checks (steps 7–9)

- Each SDK emits the same four declaration forms (required/optional path/query),
  rejects both sources/unused path names/constructor holes/invalid optionality,
  and retains existing unbound emission. TS negative type tests must fail for a
  nonexistent literal capture or an uncovered constructor, not merely at runtime.
  Effect covers both producer output and equivalent public API usage.
- WIT/schema/bridge generation is checked via repository workflows. Management
  OpenAPI/client/docs and deployed custom OpenAPI both include selector metadata;
  CLI projections and applicable output schemas retain it. Do not automatically
  bump deployment diff version without a representation/hash change.
- Run owning targeted tests before each oracle gate. Final verification rebuilds
  selected real HTTP fixtures, exercises divergent state and stream URL/root
  isolation, runs affected SDK suites, component-size report for bridge/codegen
  changes, generated drift checks, bounded bug finding, and final oracle review.
- Unit tests do not spawn subprocesses; non-CLI integration dependencies use
  `golem-test-framework`. Missing tools/artifacts are diagnosed and required local
  builds performed before tests; any irreducible validation gap is reported, not
  claimed passing. These cases are specified here, not yet executed.

Step 2 is explicitly approved by oracle. The cases above are frozen; owning-layer
tests are still to be implemented and run before their respective gates.

## Steps 3–9: gated implementation sequence

3. **Shared metadata and compiled contracts.** Add path/query selector metadata to
   authoritative `wit/deps/golem-agent/common.wit`, common base-model types, shared
   validation, agent protobuf and both WIT/protobuf conversion directions. Add
   explicit compiled selection to service-base types and customapi protobuf/codecs.
   Update in-tree producers and round-trip tests; synchronize WIT and generate
   affected SDK declarations/bindings via repository workflows. Run contract tests
   and obtain oracle approval before compiler changes.
4. **Validation and expansion.** Expand optional mounts in deployment orchestration
   before constructor binding; recompute indexes, families/IDs and OpenAPI per
   concrete alternative. Validate selector ownership/conflicts and reject only new
   optional-expansion ambiguities. Update constructor conversion, route compilation,
   and OpenAPI selector capture extraction. Keep webhooks independent. Run compiler
   and schema tests and obtain approval before runtime changes.
5. **Ordinary HTTP runtime.** Resolve selector after authentication but before
   cache revalidation/lookup. Pass UUID into existing agent-ID construction; retain
   principal/invocation/normal-creation and ephemeral admission behavior. Run
   selection, auth and ETag isolation checks and obtain approval before stream work.
6. **Durable Streams.** Reuse selection for exact root resolution, retaining named
   fork derivation, source target-root scope, path-only headers, exporter/receipt
   matching and replay. Preserve selectors in generated locations/lazy guidance;
   suffixes precede query strings. Do not add aliases, independently addressed
   source roots, or invocation/stream PUT behavior changes. Run stream-root,
   returned-link and existing fork regressions; obtain approval before SDK APIs.
7. **SDK surfaces.** Implement agreed TS/Rust/Scala/MoonBit syntax, constructor
   coverage exemption only for declared selectors, literal-name inference and
   negative tests. Include Effect public API/producer parity and shared router
   metadata. Update handwritten Scala bridge structures/codecs and regenerate TS
   declarations/templates, Scala guest templates and MoonBit ABI bindings. Run
   affected SDK tests and obtain approval before API descriptions/docs.
8. **Descriptions and docs.** Generate deployed custom OpenAPI selector parameters
   and optional alternatives; separately regenerate management OpenAPI, golem-client
   and REST docs. Update CLI deployment projection/output schemas through their
   generators. Document selection, omission, lifecycle, authorization, restrictions
   and all SDK examples, plus root-scoped DS fork-source usage. Preserve the plain
   explanation below for the eventual PR description. Obtain oracle approval.
9. **Integrated verification.** Rebuild selected fixtures and run real divergent
   original/fork HTTP and selected-root stream tests, regress all unbound modes,
   callbacks, auth/cache, SDK suites, required component-size report and generated
   drift checks. Bump diff model only if representation/hash changed. Perform
   bounded bug finding and final oracle review, addressing findings with convergent
   reruns. Report actual committed/uncommitted delivery and limitations. Do not
   push, open a PR, merge, deploy or release without additional authorization.

### Explanation to retain for the eventual PR description

Golem already creates phantom agents and reconnects to them through SDK clients.
The missing capability is selecting a known phantom UUID from an ordinary mounted
HTTP request. A request to `/decisions/d7/state?instance=P` will select phantom P
of `DecisionAgent("d7")`, rather than the original or a fresh random phantom.
Selecting P does not clone state: an unused UUID follows normal creation.

Durable Streams already implements named stream forks as phantom agents. A PUT
to `/decisions/d7/run/forks/alternative/invocations/s1/streams/events` creates or
reattaches to a phantom derived from the root identity and the name `alternative`.
The segment is a public name, not a literal phantom UUID; `/forks/P` hashes the
name again instead of selecting P directly.

The new selector resolves the root first. With `?instance=P`, the same named-fork
request operates inside P's fork namespace, using existing derivation/export code.
`Stream-Forked-From: /decisions/d7/run/invocations/s1/streams/events` refers to P's
source stream because source and target share the selected root. Without the
selector, existing root selection is unchanged. Two roots using session `s1` and
fork name `alternative` remain different agents/streams.

SDK self-forks and stream forks share underlying agent-copy machinery but are not
interchangeable operations. SDK self-fork does not automatically expose inherited
public stream sessions; DS export separately publishes its public bindings. A
known DS fork UUID can select that exact phantom and read its published sessions,
but root PUT creation/reattachment is not DS fork PUT export/retry. Further named
forks of that directly selected phantom use it as their new root namespace.

The upstream Durable Streams fork header uses the source URL's path component.
Query selection stays on the target URL; generated links retain that selector so
following them does not accidentally address the original. No new canonical path
namespace or query-bearing fork-source extension is introduced.
