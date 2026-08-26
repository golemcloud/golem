# GOL-122 Host-Call Enforcement Plan

Checkboxes are intended to be updated as implementation progresses.

## Locked decisions

- [x] Enforcement runs only during **live execution**.
- [x] Replay never evaluates permissions, expiration, or the current effective surface.
- [x] Replay restores recorded host results and treats previously executed operations as admitted.
- [x] Live denials must be replayable as normal non-trapping host-call results through the operation's compatible typed, string, or optional channel.
- [x] Policy denial never traps.
- [x] Authorization happens before any effect, quota mutation, task spawn, stream activity, or durable `Start`.
- [x] Authorization is the linearization point; later revocation does not cancel an admitted operation.
- [x] Preserve existing guest-facing WIT signatures and shared error variants whenever policy denial can be represented through an existing result, error, string, or optional channel; add a typed result only where the existing signature cannot represent denial without trapping.
- [x] Append durable payload/function variants without reordering existing persisted tags; compatibility-only payloads introduced by this unshipped change may be removed when their host call is no longer durable.
- [x] The unchanged-authority live path must perform no service/oplog I/O and acquire no async authority lock.
- [x] `PermissionTarget` has no recipient; recipient filtering already happens when deriving the holder's effective surface.

## Existing foundation

- [x] Wallet installation, transfer, derivation, revocation, and expiration.
- [x] In-memory wallet and effective-surface cache.
- [x] Invocation-scope overlay.
- [x] Card-event synchronization and authority recovery.
- [x] Wallet reconstruction during replay.
- [x] Permission-card management authorization.
- [x] WASI P3 migration.

## Live progress — 2026-08-21

Checkboxes track implementation completion. Validation that is still open is tracked separately in the status table and Milestones 12–13, so implemented work is not mistaken for unstarted work.

| Area | Current status | Evidence / next gate |
|---|---|---|
| Milestone 0 scope and matrix | Complete | The matrix below covers every P2/P3 linker family, including intentionally ungated inbound/listen operations and the registered tool-RPC authorization boundary. Functional tool dispatch belongs to GOL-35. |
| Core authorization, target normalization, authority generations | Implemented; CI full suites pending | Current-snapshot checks and the executor unit-test build pass for `golem-common`, `golem-worker-executor --lib`, and `golem-worker-executor-test-utils`. Parsed network grants and runtime targets share lowercase, trailing-dot, and IPv4 normalization. RDBMS extraction uses `sqlparser` ASTs for the PostgreSQL, MySQL, and Ignite dialects; it distinguishes source-table `Query` authority from mutation targets, handles nested queries, joins, foreign keys, `CREATE TABLE ... LIKE`, and rename destinations, and fails closed for unsupported or ambiguous AST forms. |
| Durable compatible denials and replay | Complete; CI integration rerun pending | Recorded success and denial replay without current authority, incomplete admitted-call recovery, replay-to-live synchronization, and snapshot reconstruction of an admitted secret handle passed before the final WIT compatibility refinement. Live initialization/method input that fails `SecretVerb::Hold` admission is a dedicated non-retriable invocation rejection: it atomically persists `CancelPendingInvocation` plus `Error(PermissionDenied)`, completes only that invocation as `WorkerExecutorError::PermissionDenied`/`RpcError::Denied`, and leaves the worker idle. Status reconstruction indexes the rejection result, so a crash after commit returns the same denial without reauthorization. This occurs before guest execution or handle materialization. Focused protobuf, reconstruction, classification, and admission tests pass. |
| Host wrappers | Complete exact-import audit; compatibility refinement implemented | KV, blobstore, secrets, config, oplog, environment, outbound RPC/agent/tool operations, legacy agent operations, P2/P3 filesystem, P2/P3 DNS/TCP/UDP/HTTP, WebSocket, RDBMS, and card management are wired. Existing legacy channels are reused where possible: shared config denial is `upstream("permission denied")`, WebSocket denial is `other("permission denied")`, legacy metadata/strict-resolution denial is `none`, and legacy oplog enrichment denial is its existing string error. Secret `Hold` is enforced when a handle crosses from host to guest; admitted `id`/`metadata` access remains direct and `Reveal` remains independently gated. |
| Oplog payload compatibility | Complete; CI full suites pending | The payload enum keeps all pre-project variants and function mappings in their original order. The compatibility pass removes only the unshipped secret `id`/`metadata` durable payloads and WebSocket-specific denial case made unnecessary by restoring existing WIT signatures/error variants. Prepared count-based revert payloads remain append-only. Focused binary-tag and permission-denial protobuf round-trip tests pass. |
| Integration coverage | Source coverage complete; current full-suite validation delegated to CI | The pre-compatibility implementation passed its focused `scope_cards`, worker-executor, and completed integration groups. The final compatibility pass updates the affected host probes to assert legacy `none`, existing string/error mappings, and `Hold`-at-admission semantics. No integration suite or integration test will be run locally after that pass; CI owns current-tree integration validation. |
| Performance and observability | Complete | The post-fix benchmark passes with zero allocations for stable TCP allow/deny, filesystem, KV, and both refresh cases. Medium-card TCP p50/p95 is 1.292µs/1.750µs allow and 1.167µs/1.667µs deny. |
| Mandatory closure | Complete locally; CI suites and dependency publication pending | The rejected invocation-environment design is removed. In the dedicated `wasmtime-gol122` checkout, P3 `get-environment` is async in `8c427da2e6d4cea1e47b03bc95906b0cb5bb504e`, and the P2 descriptor stream constructors are async in `85a12fa3def898a4331ec6ef2530a472404a4b80`; Golem adds no invocation/snapshot/update/revert environment state. The final WIT compatibility pass preserves legacy optional/string/shared-error channels, keeps secret `id`/`metadata` direct after `Hold` admission, and synchronizes generated Rust, TypeScript, Scala, and MoonBit bindings. Platform, Rust SDK, MoonBit, focused denial/reconstruction tests, both affected test-component rebuilds, formatting/whitespace, and final Oracle review pass. Broad unit, worker-executor, and integration suites are CI-only at the user's direction. The Wasmtime dependency branch is local-only and must be pushed before CI can fetch `85a12fa3def898a4331ec6ef2530a472404a4b80`. |

### WIT compatibility and secret-handle admission refinement — 2026-08-21

- [x] Review every permission-denial WIT change individually against its pre-project signature.
- [x] Restore shared `wasi:config/store.error` and encode denial as existing `upstream("permission denied")`.
- [x] Restore legacy `get-agent-metadata` and `resolve-agent-id-strict` option signatures; denial is indistinguishable from absence (`none`).
- [x] Restore legacy `enrich-oplog-entries` string error; denial is `"permission denied"`.
- [x] Restore secret `id` and `metadata` plain return values and remove their unneeded durable payloads.
- [x] Move `SecretVerb::Hold` to every host-to-guest handle admission boundary: secret-backed config, live initialization/method input, synchronous and future RPC results, tool success/custom-error results, and nested handles in reveal results.
- [x] Keep live initialization/method input `Hold` denial outside Wasmtime/`anyhow` trap classification: cancel and complete that invocation with the existing RPC denial while leaving the worker available for later invocations.
- [x] Keep `SecretVerb::Reveal` independent; a handle admitted with `Hold` remains inspectable and transferable without reauthorization, while revealing plaintext still requires `Reveal`.
- [x] Restore the WebSocket error variant and encode connect denial as existing `other("permission denied")`.
- [x] Regenerate/synchronize Rust, TypeScript, Scala, and MoonBit bindings and restore high-level Rust/MoonBit legacy option wrappers.
- [x] Compile the platform, Rust SDK, and MoonBit SDK after the compatibility reduction.
- [x] Update affected host-probe and integration-test source to the restored signatures and `Hold`-at-admission semantics.
- [x] Rebuild the affected `host-api-tests` and `agent-sdk-rust` test components without running integration tests. The copied release artifacts are SHA-256 `535d6c2692b9b590c3f28edcd75ff39b42e09955b1bbba0431c8b388378ca308` and `285155e77fb92e19a978c00798a80ba4a27f1d837bafcbe82674a70df1db9bf8`, respectively.
- [x] Pass the focused payload/config/WebSocket/secret-admission unit tests, plus permission-denial protobuf and crash-status reconstruction tests.
- [x] Pass final Oracle review. The initial review found queued-invocation fanout and reconstructed-error classification defects; both were fixed with focused regressions, and the follow-up verdict is `APPROVE`.

Historical pre-compatibility integration checkpoint: the complete rerun passed both PostgreSQL and SQLite worker groups
(40/40 each), both environment-deletion groups (5/5 each), both API groups (150/150 each), registry
repository tests (166/166), OIDC/session tests (28/28), and debugging tests (18/18). It was stopped after
`oplog_processor_locality_recovery` exposed that the default-deny surface incorrectly treated the
executor-created oplog-processor plugin invocation as an ordinary non-agent invocation. Oplog processors
are admitted at operator-level plugin installation and deliberately are not projected onto the per-agent
permission-card model. A transient invocation execution mode now recognizes only
`ProcessOplogEntries`, admits its ordinary host permission targets (including oplog enrichment and
callback HTTP), is visible to direct and Accessor host paths, and is reset before invocation teardown on
success, error, or panic. It changes no persisted payload or snapshot/update/revert state. A long-running
focused test was terminated after its callback wait failed, but its guest invocations ran at 04:22 while
the correction was written at 05:26–05:32, so that run is retained only as the pre-fix reproducer and is
not evidence against the current tree. The next direct focused run also failed with the original denial,
but inspection proved that it spawned `golem-worker-service` last built at 04:42 while the correction
sources were written at 06:29–06:31; unlike `cargo make integration-tests-group6`, direct `cargo test`
does not rebuild the service executables. That invalid run is recorded in
`tmp/gol122-oplog-processor-locality-recovery-post-fix.log` only to preserve the diagnosis. After
`cargo make build-bins` rebuilt the service executables, the first valid current-binary end-to-end run
passed 1/1 in 34.940s (`tmp/gol122-oplog-processor-locality-recovery-current-bins.log`). This is retained
as historical evidence, not validation of the final compatibility refinement. No integration tests will
be run locally after that refinement; completion of the current full integration suite is a CI gate.

Validation ledger (entries before the compatibility refinement are historical rather than current-tree evidence):

```text
cargo fmt --all                                                        PASS
git diff --check                                                       PASS
cargo check -p golem-common                                            PASS
cargo check -p golem-worker-executor --lib                             PASS
cargo check -p golem-worker-executor-test-utils                        PASS
cargo test -p golem-common --lib -- model::oplog::payload               PASS (46/46)
cargo test -p golem-worker-executor --lib --no-run                      PASS
cargo test -p golem-worker-executor --lib -- <7 focused tests>          PASS (7/7 authority/algebra/expiration/scheduled ownership)
cargo test -p golem-worker-executor --lib -- authorization::targets     PASS (23/23; two RDBMS extraction defects found and fixed)
cargo test -p golem-common --lib -- card::{tests,monomorphization}       PASS (16/16)
cargo test -p golem-common --lib -- card::{parsing,subsumption}_tests    PASS (199/199)
cargo test -p golem-rust --features export_golem_agentic                PASS
golem build -P release --force-build --yes (host-api-tests)             PASS
golem build -P release --force-build --yes (expanded host probes)        PASS; copied release fixture, SHA-256 f71c6316f52925bbfcb2bbd623ff9bfcc6a37f118e0a43ddfa8f75d336063c8f
cargo check -p golem-worker-executor --bench authorization              PASS
cargo bench -p golem-worker-executor --bench authorization -- --test    PASS; stable TCP allow/deny, filesystem, KV, one-event, and burst cases allocate zero
cargo test -p golem-worker-executor --test integration -- scope_cards::  PASS (16/16, including denial replay after authority revocation)
cargo test -p integration-tests --test integration -- filesystem_permissions_enforce_recipient_isolation_across_recovery PASS (PostgreSQL + SQLite)
cargo test -p golem-worker-executor --lib -- <6 authority tests>         PASS (6/6)
cargo test -p golem-worker-executor --lib -- recorded_success_replays_without_live_expiry_or_authority_inputs PASS
cargo test -p golem-worker-executor --test integration -- keyvalue::readwrite_get_returns_the_value_that_was_set PASS (2/2 sync + streamed body with explicit test host grants)
cargo test -p golem-worker-executor --test integration -- scope_cards::protected_host_families_return_typed_default_denials PASS (1/1; eventual/cache KV, blobstore, config/environment, DNS/UDP/HTTP, WebSocket, PostgreSQL/MySQL/Ignite, legacy oplog, outbound agent RPC)
cargo test -p golem-worker-executor --test integration -- scope_cards::filesystem_permissions_isolate_resource_owners PASS (1/1; matching owner writes once, foreign owner is typed-denied with no file effect)
cargo test -p golem-worker-executor --test integration -- scope_cards::concurrent_p3_operations_authorize_independently_before_backend_access PASS (1/1; concurrent allowed TCP connects exactly once, denied TCP never reaches backend)
cargo test -p golem-worker-executor --test integration -- scope_cards::denied_tool_invocation_does_not_start_the_tool_component PASS (1/1; typed denial, one authority refresh, zero tool activation)
cargo test -p golem-worker-executor --test integration -- scope_cards::secret_reveal_authorizes_before_secret_revision_lookup PASS (1/1; denied reveal performs zero revision lookups, allowed reveal performs one)
cargo test -p golem-worker-executor --test integration -- scope_cards::snapshot_restores_admitted_secret_handle_without_reauthorization PASS (1/1; snapshot reloads, reconstructs the secret handle from the recorded wallet, and performs no extra authority refresh)
cargo test -p golem-worker-executor --test integration -- scope_cards::golem_host_agent_operations_are_typed_default_deny_and_allow_when_granted PASS (1/1; listing, strict resolution, and self-fork return typed denials before effects; listing, self metadata, strict resolution, and self-fork pass their Agent gates when granted)
cargo test -p golem-worker-executor --test integration -- scope_cards::every_protected_p2_and_p3_filesystem_import_enforces_permissions PASS (1/1; every protected P2/P3 descriptor method and multi-verb open mode returns typed `NotPermitted`)
cargo test -p golem-worker-executor --test integration -- scope_cards::every_protected_network_http_and_websocket_import_enforces_permissions PASS (1/1; every protected P2/P3 DNS, TCP, UDP, HTTP, and WebSocket entry returns its typed denial)
cargo test -p golem-worker-executor --test integration -- scope_cards::every_protected_storage_config_and_secret_import_enforces_permissions PASS (1/1; every protected eventual/cache KV, blobstore, config, and secret entry returns its typed denial; found and fixed swallowed blob delete-object denials)
cargo test -p golem-worker-executor --test integration -- scope_cards::every_protected_rdbms_agent_rpc_tool_and_oplog_import_enforces_permissions PASS (1/1; connection-level PostgreSQL/MySQL/Ignite, agent, oplog, all outbound agent RPC variants, and all tool RPC variants return typed denials)
cargo test -p golem-worker-executor --test integration -- scope_cards::every_protected_rdbms_transaction_import_enforces_permissions PASS (1/1; PostgreSQL/MySQL/Ignite transaction query/query-stream/execute honor negative table grants before backend execution; commit/rollback inherit admitted transaction authority)
cargo test -p golem-worker-executor --test integration -- scope_cards::remaining_host_facing_permission_classes_allow_their_backends PASS (1/1; environment, KV, config, and oplog grants reach their backends; all three granted tool modes pass authorization and reach the current unavailable-backend result rather than `Denied`)
cargo test -p golem-worker-executor --test integration -- <4 wrapper contract tests> PASS (4/4; KV/blob batches have zero partial effects, cache vacancies and blob read streams retain admission across revocation, new work is denied, and remaining class allows still reach backends)
cargo test -p golem-worker-executor --lib -- durable_host::authorization::targets::tests::kv_and_blob_targets_preserve_utf8_resource_names --exact --report-time PASS (1/1; typed targets preserve dotted and non-ASCII resource names)
cargo test -p golem-worker-executor --test integration -- scope_cards::blobstore_authorization_preserves_valid_utf8_container_names --report-time PASS (1/1; a WIT-valid dotted container name reaches the backend)
cargo test -p golem-worker-executor --lib -- <hostname and SQL target regressions> --report-time PASS (3/3; grant/runtime hostname normalization, ALTER rename destination preflight, and INSERT-SELECT source Query authority)
cargo test -p golem-common --lib -- card::parsing_tests card::subsumption_tests card::rendering_tests --report-time PASS (201/201)
cargo test -p golem-common -- card::parsing_tests --report-time            PASS (96/96; trailing-dot/lowercase hostname normalization and malformed-label rejection)
cargo test -p golem-worker-executor --lib -- authorization::targets       PASS (33/33; dialect AST extraction, joins/subqueries, DDL/FK sources, fail-closed wrappers, fallback, and normalized network targets)
sdks/moonbit/golem_sdk/scripts/regen-bindings.sh                           PASS (repository-pinned Golem wit-bindgen revision 4407232)
moon fmt --check (sdks/moonbit/golem_sdk)                                  PASS
moon check --target wasm (sdks/moonbit/golem_sdk), initial                 FAIL (7 hand-maintained config and scheduled-RPC wrapper type errors; fixed, then webhook typed-result use fixed)
moon check --target wasm (sdks/moonbit/golem_sdk), final                   PASS
moon build --target wasm (sdks/moonbit/golem_sdk)                          PASS
moon test --target wasm (sdks/moonbit/golem_sdk)                           PASS (433/433)
cargo test -p golem-registry-service --lib -- agent_initial_card_inherits_parent_ids_from_creator_surface PASS (1/1)
cargo test -p golem-common --lib -- model::oplog::payload --report-time     PASS (46/46, pure GOL-122 extraction)
cargo test -p golem-worker-executor --lib -- durable_host::tool --report-time PASS (4/4, pure GOL-122 extraction)
cargo test -p golem-worker-executor --test integration -- <3 tool permission tests> PASS (3/3, pure GOL-122 extraction; denied/all three modes/granted unavailable backend)
cargo fmt --all -- --check (dedicated wasmtime-gol122 checkout)           PASS
cargo check -p wasmtime-wasi --features p3 (dedicated checkout)          PASS
git diff --check (dedicated wasmtime-gol122 checkout)                    PASS
cargo test -p wasmtime-wasi --features p3 --lib                          BLOCKED before test execution by the fork's test-program artifact builder under Cargo 1.97 (`CARGO_BUILD_BUILD_DIR` places the adapter outside its asserted target path); the affected crate check passes
cargo check -p golem-worker-executor --lib (async P3 fork integration)   PASS
cargo check -p golem-worker-executor --lib (portable git dependency on Wasmtime commit 8c427da2e) PASS
golem build -P release --force-build --yes (host-api-tests environment fixture) PASS; component SHA-256 e33710a42fb0ae314edccc72b8b41cf3d55229f0d2a9941345dff09d3b66bdb4
cargo test -p golem-worker-executor --test integration -- scope_cards::p2_and_p3_environment --report-time PASS (2/2; each test exercises P2 and P3 revocation-at-boundary and recorded-result replay)
cargo test -p golem-worker-executor --test integration -- scope_cards::protected_host_families_return_typed_default_denials --report-time PASS (1/1 after explicitly preserving the fixture's initialization-only self-view grant; all tested host families remain default-denied)
cargo fmt --all -- --check; git diff --check (post-environment refinement) PASS
cargo make unit-tests (post-environment refinement, portable Wasmtime dependency) PASS (3,545 passed, 2 ignored, 0 failed across 17 suites)
cargo check -p golem-worker-service -p golem-debugging-service -p golem-registry-service PASS (corrected Agent resource mappings and debug-alias expansion compile against the portable Wasmtime dependency)
cargo test -p golem-common --lib -- card::parsing_tests card::rendering_tests card::subsumption_tests --report-time PASS (209/209; includes empty-vs-wildcard Agent resources, verb-specific resources, and parse-time-only legacy debug expansion through card and scope-card deserialization)
cargo fmt --all -- --check; git diff --check (post-Agent-model correction) PASS
cargo test -p golem-worker-executor --lib -- durable_host::wasm_rpc::tests --report-time PASS (15/15 after append-only first-activation fingerprint correction)
cargo test -p golem-common --lib -- model::oplog::payload --report-time PASS (48/48; appended activation request/response/function variants and existing binary tags)
cargo test -p golem-worker-executor --test integration -- scope_cards::every_protected_rdbms_agent_rpc_tool_and_oplog_import_enforces_permissions --report-time PASS (1/1 after activation-decision correction; outbound invoke, async invoke-and-await, invoke-and-await, and schedule denials remain typed and pre-effect)
cargo test -p golem-worker-executor --test integration -- scope_cards::scope_card_delivery_and_cleanup_survive_crash_replay scope_cards::outbound_rpc_denial_replays_without_activating_the_target --report-time PASS (2/2; persisted admitted fingerprint, exactly one activation record after restart, idempotent target dispatch, denied target never created, denial replay uses no live authority)
cargo test -p golem-worker-service --lib -- <4 exact target tests> --report-time PASS (4/4; concrete invocation method, lifecycle empty resources, oplog range, filesystem paths and verbs, cancellation identifier, plugin name, and revert cutoff)
cargo test -p golem-debugging-service --lib -- debugging_requires_every_permission_in_the_legacy_alias_expansion --report-time PASS (1/1; all eight canonical constituent permissions are required)
cargo make fix (workspace and dev-tools, all targets)                   PASS
cargo fmt --all -- --check; git diff --check (post-fix)                PASS
cargo test -p golem-common --lib -- model::oplog::payload --report-time PASS (49/49; includes appended WebSocket-error binary-tag regression)
cargo test -p golem-worker-executor --lib -- durable_host::wasm_rpc::tests --report-time PASS (15/15 post-fix)
cargo test -p golem-worker-service --lib -- <4 exact target tests> --report-time PASS (4/4 post-fix)
cargo test -p golem-debugging-service --lib -- debugging_requires_every_permission_in_the_legacy_alias_expansion --report-time PASS (1/1 post-fix)
cargo test -p golem-worker-executor --test integration -- <3 RPC authorization/replay tests> --report-time PASS (3/3 post-fix; all five RPC forms denied before activation, admitted and denied restart paths pass)
cargo check -p golem-worker-executor -p golem-worker-service -p golem-debugging-service -p golem-worker-executor-test-utils -p golem-api-grpc PASS (post-fix)
cargo make unit-tests (first final run)                                FAIL (one stale registry test fixture still used wildcard resources for `resume` and `update-revision`; production code was not implicated)
cargo test -p golem-registry-service --lib -- agent_initial_card_inherits_parent_ids_from_creator_surface --report-time PASS (1/1 after converting the fixture to canonical empty lifecycle resources)
cargo make unit-tests (final rerun)                                    PASS (3,562 passed, 2 ignored, 0 failed across 17 suites)
cargo make worker-executor-tests (first final run)                     FAIL (771 passed, 41 failed, 4 ignored; stale test-component artifacts account for the missing `golem:agent/host@2.0.0` linker imports, with additional snapshot-authority, permission-fixture, SQL-fixture, timing, memory, and expected-oplog-count failures to diagnose before the mandatory rerun)
npx pnpm install; npx pnpm run build; npx pnpm run build-agent-template PASS (TypeScript SDK and embedded agent template rebuilt after the host WIT changes)
test-components/build-components.sh rebuild ts                       PASS (all five TypeScript test applications rebuilt and copied against the current SDK/template)
golem build -P release --force-build --yes; golem exec -P release copy (agent-counters) PASS (snapshot fixture rebuilt against the current Rust SDK/WIT)
cargo test -p golem-worker-executor --test integration -- <18 first-run failure regressions> FAIL (17 passed, 1 failed; the rebuilt fixtures, snapshot authority restoration, SQL setup, timing probes, memory expectation, and oplog counts pass; cross-component strict agent resolution exposed an over-restrictive owner-target shortcut, now corrected and under focused rerun)
cargo test -p golem-worker-executor --test integration -- api::resolve_components_from_name --report-time PASS (1/1; cross-component strict resolution builds the canonical target owner and honors the matching grant before checking agent existence)
cargo make worker-executor-tests (complete rerun)                     PASS (786 passed, 4 ignored, 0 failed; post-fixture and strict-resolution correction)
Final semantic replay audit                                           PASS (an incomplete persisted Start remains admitted; no live authority recheck is allowed during repair)
cargo check -p golem-worker-service -p golem-worker-executor -p golem-debugging-service --tests PASS (prepared count-revert protocol)
cargo test -p golem-worker-service --lib -- revert_uses_the_concrete_cutoff_for_index_and_count_targets PASS (1/1)
cargo test -p golem-worker-executor --test integration -- <2 prepared count-revert tests> PASS (2/2; ordinary revert and stale-tip rejection with no oplog write)
cargo test -p golem-common --lib model::oplog::payload::tests          PASS (49/49; includes prepared/unprepared revert round trips)
cargo test -p golem-rust --features export_golem_agentic              PASS (all runtime, integration, and doc-test groups)
cargo test -p golem-rust-macro                                        PASS (97/97; doc tests also pass)
cargo fmt --all -- --check; git diff --check (post-prepared-revert)    PASS
cargo make integration-tests (first mandatory run)                    FAIL (39/40 in group 1; durable config access adds one expected imported-function oplog entry, so the stale assertion was corrected from 2 to 3)
cargo make integration-tests (second mandatory run)                   FAIL (group 1 passed 40/40 and environment deletion passed 5/5; registry service stack-overflowed in group 2 while decoding recursive component metadata, and the remaining 140 failures were connection-error cascades)
cargo test -p integration-tests --test integration -- add_new_agent_config_entry_during_update_postgres PASS (1/1 without `RUST_MIN_STACK` after setting the registry Tokio worker stack to 4 MiB)
cargo fmt --all -- --check; git diff --check (post-stack fix)           PASS
cargo make build-bins (post-oplog-processor correction)                PASS
cargo test -p integration-tests --test sharding -- oplog_processor_locality_recovery --report-time PASS (1/1 in 34.940s; freshly rebuilt service executables)
cargo make integration-tests (final local attempt)                    STOPPED by user after all completed groups passed, including worker 80/80, environment deletion 10/10, API 300/300, registry 166/166, OIDC/session 28/28, debugging 18/18, sharding 9/9, and plugin/OTLP 15/15; remaining groups move to CI
cargo make fix; cargo fmt --all -- --check; git diff --check (final)   PASS
cargo test -p golem-common --lib -- model::oplog::payload --report-time PASS (49/49 after WIT compatibility reduction)
cargo test -p golem-worker-executor --lib -- permission_denial --report-time PASS (2/2 existing config/WebSocket error mappings)
cargo test -p golem-worker-executor --lib -- secret_hold_admission --report-time PASS (2/2 recursive handle discovery and invalid-snapshot rejection)
cargo test -p golem-worker-executor --lib -- permission_denied_is_a_non_retriable_invocation_rejection --report-time PASS (1/1 typed denial, invocation-rejection classification, no retry)
cargo test -p golem-common --lib -- main_payload_additions_keep_existing_p3_binary_tags_stable --report-time PASS (1/1 after final payload reduction)
cargo test -p golem-common --lib -- agent_error_permission_denied_protobuf_roundtrip --report-time PASS (1/1 exact durable error wire round trip)
cargo test -p golem-worker-executor --lib -- permission_denied_rejection_survives_status_reconstruction --report-time PASS (1/1 atomic cancellation/error reconstruction leaves the worker idle and indexes the denial)
cargo test -p golem-worker-executor --lib -- reconstructed_permission_denial_keeps_its_executor_error_type --report-time PASS (1/1 restarted lookup preserves `WorkerExecutorError::PermissionDenied`)
cargo test -p golem-worker-executor --lib -- invocation_rejection_fails_only_the_rejected_pending_key --report-time PASS (1/1 later queued invocations remain pending)
cargo check -p golem-api-grpc -p golem-common -p golem-worker-executor PASS (final denial durability changes)
golem build/exec copy -P release --force-build --yes (host-api-tests and agent-sdk-rust only) PASS; no integration tests run
Oracle final follow-up                                                   APPROVE (exact-one rejection and restarted denial equivalence)
```

The final registered-import audit traced every P2/P3 and Golem linker registration to its implementation. Every authority-crossing operation authorizes before backend access, quota mutation, durable `Start`, task/resource creation, body consumption, or transport; lifecycle/plumbing operations inherit a previously admitted resource or are explicitly ungated. No missing or post-effect enforcement point was found. The test framework now grants the complete host surface explicitly for legacy behavior tests, while enforcement tests opt out per agent type to preserve default-deny coverage.

SQL review checkpoint: the previously reported SQL-review bug-finder non-convergence remains parked as
requested. Do not rerun or override that review until the separate follow-up decision.

Current validation queue:

- [x] Rerun `cargo make unit-tests` after the final environment payload refinement: 3,545 passed,
  2 ignored, 0 failed across 17 suites.
- [x] Compile the corrected Agent resource mappings and parse-time-only debug alias expansion across
  `golem-worker-service`, `golem-debugging-service`, and `golem-registry-service`.
- [x] Run the complete focused card parsing/rendering/subsumption suite after the Agent model correction:
  209 passed, 0 failed.
- [x] Pass focused admitted and denied outbound-RPC activation crash/replay tests, including persisted
  fingerprints, no duplicate activation record, no target activation for denial, and enqueue-time
  idempotency under recovery: 2 passed, 0 failed.
- [x] Add and pass focused service-level tests for the corrected oplog, cancellation, plugin, filesystem,
  lifecycle, and concrete debugging-permission targets: 5 passed, 0 failed.
- [x] Run the mandatory workspace lint/fix gate and recheck formatting and patch whitespace.
- [x] Rerun `cargo make unit-tests` after the final Agent model and service-target correction:
  3,562 passed, 2 ignored, 0 failed across 17 suites.
- [x] Run `cargo make worker-executor-tests` in the GOL-122 checkout. The first final run completed
  with 771 passed, 41 failed, and 4 ignored. Rebuild the affected test components, diagnose every
  remaining non-artifact failure, and rerun the complete suite; do not close from the partial result.
  The first focused regression run passed 17/18; its sole failure showed that legacy agent-operation
  authorization default-denied every cross-component target instead of constructing the target's
  canonical owner. The helper now resolves component metadata before the target-agent existence lookup,
  preserving pre-effect authorization while allowing a matching cross-component grant; the exact
  regression passes 1/1. The complete rerun passes 786 tests with 4 ignored and 0 failed.
- [x] Replace wildcard authorization for count-based revert with a concrete prepared cutoff, preserve
  that preparation in the durable request for incomplete replay repair, reject stale observed tips
  under the worker instance lock before mutation, and pass the focused service/executor/payload tests.
- [x] Run the Rust SDK runtime and macro test suites after propagating compatible host denials through
  the public Rust SDK APIs, including the preserved legacy option wrappers.
- [ ] Complete `cargo make integration-tests` in CI and close only from passing evidence.
  The first run exposed and corrected the expected imported-function oplog count added by durable config
  access. The next run passed group 1 (40/40) and environment deletion (5/5), then the registry process
  stack-overflowed while decoding recursive component metadata in group 2. Raising only the registry's
  Tokio worker stack from 2 MiB to 4 MiB fixes the exact failing test without an environment override.
  The current complete rerun has passed PostgreSQL and SQLite worker groups (40/40 each),
  environment-deletion groups (5/5 each), and API groups (150/150 each), including the former
  stack-overflow reproducer, as well as registry repository (166/166), OIDC/session (28/28), and
  debugging (18/18). It was stopped after the oplog-processor locality-recovery test exposed the
  operator-authority gap described in the latest checkpoint. The narrow invocation-mode correction is
  implemented. The first attempted focused run exercised a binary built before that correction and is
  only a pre-fix reproducer. After rebuilding service executables, the exact current-binary regression
  passes 1/1 in 34.940s. The final local run subsequently passed every completed group listed in the
  status table before it was stopped at the user's request; the remaining groups move to CI.
- [ ] Rerun `cargo make worker-executor-tests` and `cargo make unit-tests` in CI. Complete local reruns
  were explicitly waived after the prior 786/4 and 3,562/2 passing runs.
- [x] Run the final focused formatting and patch-whitespace gates after the compatibility refinement:
  `cargo fmt --all -- --check` and `git diff --check` pass. The broad workspace fix gate is CI-only.
- [ ] Push the Wasmtime branch containing `85a12fa3def898a4331ec6ef2530a472404a4b80` before starting CI.
- [x] Amend the final compatibility refinement into the single local Golem commit without pushing.

## Milestone 0 — Freeze the enforcement matrix

Before writing interception code, enumerate every host import registered by the executor and classify it as:

1. Protected semantic operation.
2. Resource lifecycle/plumbing operation.
3. Already protected elsewhere.
4. Out of GOL-122 scope.

- [x] Create a working matrix with columns:
  - interface/function
  - permission class
  - owner
  - verb
  - resource
  - semantic authorization point
  - compatible non-trapping denial result
  - durable-result behavior
  - replay behavior
  - effect that must not begin before authorization
- [x] Audit all P2 and P3 linker registrations, not only currently obvious wrappers.
- [x] Explicitly mark drops, polls, local state inspection, and resource getters that need no gate.
- [x] Confirm that inbound RPC checks remain defense in depth but do not replace outbound caller enforcement.

### Scope decisions to close

- [x] **Network:** current [`NetworkVerb`](golem-common/src/base_model/card/class/network.rs) only has `Connect`.
  - Default mapping: DNS, TCP connect, UDP remote destination, and HTTP dispatch use `Connect`.
  - Decide whether bind/listen are intentionally outside this model or whether new `Bind`/`Listen` verbs must be added.
- [x] **Environment:** make the standard P3 environment host call async in the Golem Wasmtime fork and enforce at that durable host-call boundary as described in Milestone 7.
- [x] **RDBMS:** decide whether GOL-122 includes SQL-to-table target extraction.
- [x] **Tools:** confirm whether a functional tool invocation host boundary currently exists.
- [x] Record RDBMS/tools as explicitly included or explicitly deferred; do not leave them ambiguous.

### Closed scope decisions

Network authorization is outbound-only and uses the existing `NetworkVerb::Connect`. DNS resolution,
TCP connect, UDP connect or an unconnected send destination, WebSocket connect, and HTTP dispatch are
gated. Bind, listen, accept, socket creation/options/address inspection, receive, and established-stream
I/O are intentionally ungated: they do not initiate access to external authority. A connected socket or
stream inherits the admission of the operation that established it; revocation after admission does not
cancel it. `PermissionTarget` never contains a recipient.

Environment uses an async standard P2/P3 host call. The private Wasmtime fork marks P3
`wasi:cli/environment.get-environment` async, allowing both previews to build the established enriched
environment, authorize `EnvClass / EnvVerb::Read / <variable-name>` at the host-call boundary, and
durably record the filtered result. Replay returns the recorded result without live authorization.
Invocation start and snapshot/update/revert environment state remain untouched. Arguments and initial
cwd remain ungated.

RDBMS enforcement and tool-RPC enforcement are included, with no deferred/TBD GOL-122 surface. RDBMS
must parse each statement before any connection use and authorize every referenced
database/schema/table (`RdbmsVerb::Query` for reads, `RdbmsVerb::Mutate` for writes and DDL); statements
whose complete target set cannot be established fail closed. `golem:tool/host` is registered, but its
invocation backend is intentionally owned by [GOL-35](https://linear.app/golem-cloud/issue/GOL-35/implementation-of-the-tool-host-function-call-on-top-of-side-car).
GOL-122 resolves the bound tool and canonical command arguments, gates `tool-rpc.invoke`,
`async-invoke-and-await`, and `invoke-and-await` with `ToolVerb::Invoke`, and durably records and replays
typed permission denials. A granted call reaches the current unavailable-backend
`RemoteInternalError`; GOL-122 does not implement GOL-35 runtime dispatch.

Inbound direct-invocation checks remain defense in depth. They do not replace caller-side authorization:
the outbound RPC/agent/tool wrapper must check the calling agent's wallet and invocation overlay before
lookup, activation, scheduling, or dispatch.

### Registration-to-handler audit matrix

This matrix is exhaustive for the registrations in `wasi_host::create_linker`, the P3 bulk registration
it calls, and the registrations appended by `Bootstrap::create_wasmtime_linker`. Names are exact WIT
interface/function names; semicolon-separated functions in one row share one unambiguous disposition.
For gated rows, the owner is the class's owner from the target mapping below and the resource is shown
after the verb. “Local/plumbing” includes resource drops and future/stream getters after an operation has
already been admitted.

| Preview | Exact WIT interface / function(s) | Implementation file(s) | Permission mapping or explicit ungated reason |
|---|---|---|---|
| P3 | `wasi:cli/environment.get-environment` | Golem Wasmtime fork `crates/wasi/src/p3/{bindings.rs,cli/host.rs}`, `durable_host/p3/cli.rs`, `durable_host/cli/environment.rs` | **Env / `Read` / each variable name.** The fork makes the standard P3 host call async; authorize/filter one complete enriched view before returning and durably record that filtered result. |
| P3 | `wasi:cli/environment.get-arguments`; `get-initial-cwd` | `durable_host/p3/cli.rs` | Ungated CLI arguments/cwd; not environment authority. |
| P3 | `wasi:cli/exit.exit`; `exit-with-code` | `durable_host/p3/cli.rs` | Ungated process-control plumbing. |
| P3 | `wasi:cli/stdin.read-via-stream`; `wasi:cli/stdout.write-via-stream`; `wasi:cli/stderr.write-via-stream` | `durable_host/p3/cli.rs` | Ungated worker stdio/log capture; no gated external operation. |
| P3 | `wasi:cli/terminal-input.[drop]`; `terminal-output.[drop]`; `terminal-stdin.get-terminal-stdin`; `terminal-stdout.get-terminal-stdout`; `terminal-stderr.get-terminal-stderr` | `durable_host/p3/cli.rs` | Ungated terminal getters/resource lifecycle. |
| P3 | `wasi:clocks/types` conversions; `system-clock.now`; `get-resolution`; `monotonic-clock.now`; `get-resolution`; `wait-until`; `wait-for` | `durable_host/p3/clocks.rs` | Ungated clock/wait plumbing. |
| P3 | `wasi:random/random.get-random-bytes`; `get-random-u64`; `insecure.get-insecure-random-bytes`; `get-insecure-random-u64`; `insecure-seed.get-insecure-seed` | `durable_host/p3/random.rs` | Ungated random plumbing. |
| P3 | `wasi:filesystem/preopens.get-directories`; `types.convert-error-code`; `descriptor.[drop]` | `durable_host/p3/filesystem.rs` | Ungated preopen/error/resource plumbing; preopens expose guest paths, not backing paths. |
| P3 | `wasi:filesystem/types.descriptor.read-via-stream` | `durable_host/p3/filesystem.rs` | **Filesystem / `Read` / canonical absolute descriptor path.** |
| P3 | `descriptor.write-via-stream`; `append-via-stream`; `set-size`; `set-times`; `sync-data`; `sync` | `durable_host/p3/filesystem.rs` | **Filesystem / `Write` / canonical absolute descriptor path.** Admission is inherited by the resulting stream/task. |
| P3 | `descriptor.read-directory` | `durable_host/p3/filesystem.rs` | **Filesystem / `List` / canonical absolute directory path.** |
| P3 | `descriptor.stat`; `stat-at`; `metadata-hash`; `metadata-hash-at`; `readlink-at` | `durable_host/p3/filesystem.rs` | **Filesystem / `Stat` / canonical absolute path (descriptor-relative argument resolved first).** |
| P3 | `descriptor.create-directory-at`; `set-times-at`; `open-at` when create/write/truncate; `symlink-at` | `durable_host/p3/filesystem.rs` | **Filesystem / `Write` / resolved destination path.** `open-at` also preflights `Read`/`List` as requested by flags/type. |
| P3 | `descriptor.open-at` when read/enumerate | `durable_host/p3/filesystem.rs` | **Filesystem / `Read` or `List` / resolved opened path.** All requested verbs are preflighted together. |
| P3 | `descriptor.remove-directory-at`; `unlink-file-at` | `durable_host/p3/filesystem.rs` | **Filesystem / `Delete` / resolved target path.** |
| P3 | `descriptor.rename-at` | `durable_host/p3/filesystem.rs` | **Filesystem / `Delete` / source + Filesystem / `Write` / destination.** |
| P3 | `descriptor.link-at` | `durable_host/p3/filesystem.rs` | **Filesystem / `Read` / source + Filesystem / `Write` / destination.** |
| P3 | `descriptor.advise`; `get-flags`; `get-type`; `is-same-object` | `durable_host/p3/filesystem.rs` | Ungated local descriptor state/advisory inspection; no filesystem data or namespace effect. |
| P3 | `wasi:sockets/ip-name-lookup.resolve-addresses` | `durable_host/p3/sockets/dns.rs` | **Network / `Connect` / normalized hostname with `PortPattern::Any`.** Gate before DNS. |
| P3 | `wasi:sockets/types.tcp-socket.connect` | `durable_host/p3/sockets/tcp.rs` | **Network / `Connect` / normalized remote host and port.** Connection and derived streams inherit admission. |
| P3 | `tcp-socket.bind`; `listen`; `create`; all `get-*`/`set-*` socket options; `[drop]` | `durable_host/p3/sockets/tcp.rs` | Ungated local bind/listen/options/address inspection/resource lifecycle; no outbound authority crossing. |
| P3 | `tcp-socket.send`; `receive` | `durable_host/p3/sockets/tcp.rs` | Ungated established-stream I/O; inherits successful `connect` admission. |
| P3 | `wasi:sockets/types.udp-socket.connect`; `send` with a new/unconnected destination | `durable_host/p3/sockets/udp.rs` | **Network / `Connect` / normalized destination host and port.** Connected sends inherit admission; each unconnected destination is admitted before send. |
| P3 | `udp-socket.bind`; `create`; `disconnect`; all `get-*`/`set-*` options; `receive`; `[drop]` | `durable_host/p3/sockets/udp.rs` | Ungated local setup/options/receive/resource lifecycle; no new outbound destination. |
| P3 | `wasi:http/client.send` | `durable_host/p3/http/send.rs` | **Network / `Connect` / normalized final URI host and effective port.** Each redirect destination is separately admitted. |
| P3 | `wasi:http/types.fields.*`; `request.new`; request `get-*`/`set-*`; `request-options.*`; response `new`/`get-*`/`set-*`; request/response `consume-body`; all `[drop]`; error conversions | `durable_host/p3/http/host_types.rs`, `request_body.rs`, `response_body.rs` | Ungated HTTP object/body/resource plumbing. Outbound authority is crossed only by `client.send`; inbound HTTP objects are intentionally ungated. |
| P2 | `wasi:cli/environment.get-environment`; `get-arguments`; `initial-cwd` | `durable_host/cli/environment.rs` | Environment variables: **Env / `Read` / variable**, authorized/filtered and recorded at the async host-call boundary; args/cwd ungated CLI data. |
| P2 | `wasi:cli/exit.exit`; stdin/stdout/stderr `get-*`; terminal-input/output `[drop]`; terminal-stdin/stdout/stderr `get-terminal-*` | `durable_host/cli/{exit,stdin,stdout,stderr,terminal_input,terminal_output,terminal_stdin,terminal_stdout,terminal_stderr}.rs` | Ungated CLI/terminal plumbing and lifecycle. |
| P2 | `wasi:clocks/monotonic-clock.now`; `resolution`; `subscribe-duration`; `subscribe-instant`; `wall-clock.now`; `resolution` | `durable_host/clocks/{monotonic_clock,wall_clock}.rs` | Ungated clock/poll plumbing. |
| P2 | `wasi:io/error.to-debug-string`; `[drop]`; `poll.poll`; pollable `ready`; `block`; `[drop]`; streams input/output operations, `subscribe`, `[drop]` | `durable_host/io/{error,poll,streams}.rs` | Ungated I/O plumbing. A stream created by a gated semantic operation carries that operation's admission. |
| P2 | `wasi:random/random.get-random-bytes`; `get-random-u64`; `insecure.get-insecure-random-bytes`; `get-insecure-random-u64`; `insecure-seed.insecure-seed` | `durable_host/random/{random,insecure,insecure_seed}.rs` | Ungated random plumbing. |
| P2 | `wasi:filesystem/preopens.get-directories`; `types.descriptor.[drop]` | `durable_host/filesystem/{preopens,types}.rs` | Ungated preopen/resource lifecycle. |
| P2 | `descriptor.read`; `read-via-stream` | `durable_host/filesystem/types.rs` | **Filesystem / `Read` / canonical absolute descriptor path.** |
| P2 | `descriptor.write`; `write-via-stream`; `append-via-stream`; `set-size`; `set-times`; `sync-data`; `sync` | `durable_host/filesystem/types.rs` | **Filesystem / `Write` / canonical absolute descriptor path.** |
| P2 | `descriptor.read-directory` | `durable_host/filesystem/types.rs` | **Filesystem / `List` / canonical absolute directory path.** |
| P2 | `descriptor.stat`; `stat-at`; `metadata-hash`; `metadata-hash-at`; `readlink-at` | `durable_host/filesystem/types.rs` | **Filesystem / `Stat` / canonical resolved path.** |
| P2 | `descriptor.create-directory-at`; `set-times-at`; `open-at`; `symlink-at`; `remove-directory-at`; `unlink-file-at`; `rename-at`; `link-at` | `durable_host/filesystem/types.rs` | Same multi-target **Write/Delete/Read** filesystem mapping as the corresponding P3 rows; all targets preflight together. |
| P2 | `descriptor.advise`; `get-flags`; `get-type`; `is-same-object` | `durable_host/filesystem/types.rs` | Ungated local descriptor/advisory inspection. |
| P2 | `wasi:sockets/ip-name-lookup.resolve-addresses` | `durable_host/sockets/ip_name_lookup.rs` | **Network / `Connect` / normalized hostname with `PortPattern::Any`.** |
| P2 | `wasi:sockets/tcp.start-connect`; `finish-connect` | `durable_host/sockets/tcp.rs` | **Network / `Connect` / normalized remote host and port**, admitted at `start-connect`; resulting streams inherit admission. |
| P2 | TCP `start-bind`; `finish-bind`; `start-listen`; `finish-listen`; `accept`; create, options/address getters/setters, `subscribe`, `shutdown`, `[drop]`; `instance-network.instance-network`; `network.error-code` | `durable_host/sockets/{tcp,tcp_create_socket,instance_network,network}.rs` | Ungated local bind/listen/accept/socket options/network handle/resource plumbing. |
| P2 | `wasi:sockets/udp.start-bind`; `finish-bind`; `stream`; outgoing-datagram-stream `send` | `durable_host/sockets/udp.rs` | `stream` with remote and every datagram destination: **Network / `Connect` / normalized host and port** before outbound send; bind is ungated. |
| P2 | UDP create/options/address getters/setters; incoming stream `receive`; all `subscribe`/`check-send`/`[drop]` | `durable_host/sockets/{udp,udp_create_socket}.rs` | Ungated local/options/receive/poll/resource plumbing; connected outgoing stream inherits admission. |
| P2 | `wasi:http/outgoing-handler.handle` | `durable_host/http/outgoing_http.rs` | **Network / `Connect` / normalized final URI host and effective port**; redirects separately gated. |
| P2 | `wasi:http/types` fields/request/options/response/body/future methods and drops | `durable_host/http/types.rs` | Ungated object, body, future, inbound HTTP, and resource plumbing; outbound gate is `handle`. |
| P2 | `wasi:blobstore/blobstore.create-container`; `get-container`; `delete-container`; `container-exists`; `copy-object`; `move-object` | `durable_host/blobstore/mod.rs` | **Blob / `Write`, `Read`, `Delete` or `List` / exact bucket/key** as applicable; copy = source Read + destination Write, move also source Delete. |
| P2 | `wasi:blobstore/container.name`; `info` | `durable_host/blobstore/container.rs` | Ungated local metadata on a container handle that can only be obtained through admitted `create-container` or `get-container`; no backend access. |
| P2 | `wasi:blobstore/container.get-data`; `write-data`; `delete-object(s)`; `has-object`; `object-info`; `clear`; `list-objects` | `durable_host/blobstore/container.rs` | **Blob / Read, Write, Delete, or List / exact bucket/key or prefix.** All batch targets preflight. |
| P2 | `wasi:blobstore/types` incoming/outgoing value creation, consume/write/size and drops | `durable_host/blobstore/types.rs` | Ungated value/stream plumbing; it inherits admission from the blob operation that consumes or creates it. |
| P2 | `wasi:keyvalue/types.bucket.open-bucket` | `durable_host/keyvalue/types.rs` | Ungated local bucket handle creation; no backend access until a semantic KV operation. Other types methods/drops are plumbing. |
| P2 | `wasi:keyvalue/eventual.get`; `exists`; `set`; `delete`; `eventual-batch.get-many`; `keys`; `set-many`; `delete-many` | `durable_host/keyvalue/{eventual,eventual_batch}.rs` | **KV / Read, Write, Delete, or List / exact store/key or prefix**; batches preflight all keys. |
| P2 | `wasi:keyvalue/cache.get`; `exists`; `set`; `get-or-set`; `delete` | `durable_host/keyvalue/caching.rs` | **KV / Read, Write, Delete / exact store/key.** `get-or-set` preflights Read+Write. |
| P2 | `wasi:keyvalue/cache` future `get`/drops; vacancy `fill`/drop | `durable_host/keyvalue/caching.rs` | Ungated admitted plumbing. A `get-or-set` future or vacancy cannot exist without its original admission; the vacancy retains that permit through `fill` or drop and does not reauthorize after revocation. |
| P2 | `wasi:keyvalue/wasi-keyvalue-error.error.trace`; `[drop]` | `durable_host/keyvalue/error.rs` | Ungated local typed-error inspection/lifecycle. |
| P2 | `wasi:logging/logging.log` | `durable_host/logging/logging.rs` | Ungated logging by settled policy; must not include secrets/permission targets. |
| P2 | `wasi:config/store.get`; `get-all` | `durable_host/config/mod.rs` | **Config / `Read` / exact key**; `get-all` preflights/materializes only individually admitted keys. Denial uses existing `error.upstream("permission denied")`. |
| P2 | `golem:secrets/types.id`; `metadata`; `golem:secrets/reveal.reveal`; secret `[drop]` | `durable_host/secrets/mod.rs` | `reveal`: **Secret / `Reveal` / canonical key/version**. `id`/`metadata` are ungated inspection of an already admitted handle. **Secret / `Hold` / canonical config key** is checked before any host-to-guest handle transfer; drop is ungated. |
| P2 | `golem:rdbms/{postgres,mysql,ignite2}.connection.open` | `durable_host/rdbms/{postgres,mysql,ignite}.rs`, `rdbms/mod.rs` | Ungated parse-only local handle creation; it opens no connection, pool, client, or socket. The first statement or transaction begin performs admission before connection use. |
| P2 | `golem:rdbms/{postgres,mysql,ignite2}` connection/transaction `query`; `query-stream`; `execute`; `begin-transaction`; `commit`; `rollback` | `durable_host/rdbms/{postgres,mysql,ignite}.rs`, `rdbms/mod.rs` | **RDBMS / `Query` or `Mutate` / every parsed database/schema/table**; fail closed before connection use if complete extraction is impossible. Transaction commit/rollback inherit the transaction admission. |
| P2 | RDBMS result-stream `get-columns`; `get-next`; lazy value/type `new`; `get`; all RDBMS `[drop]` | same RDBMS files | Ungated admitted-result and local resource plumbing; no new SQL operation. |
| P2 | `golem:websocket/client.connect` | `durable_host/websocket/client.rs` | **Network / `Connect` / normalized URI host and effective port.** Denial uses existing `error.other("permission denied")`. |
| P2 | WebSocket connection `send`; `receive`; `receive-with-timeout`; `close`; `[drop]` | `durable_host/websocket/client.rs` | Ungated established connection I/O/lifecycle; inherits connect admission. |
| P2 | `golem:quota/types` token/reservation methods | `durable_host/quota/mod.rs` | Explicitly ungated quota-token plumbing; quota authority is carried by the token and validated by its existing service/lease rules, not a GOL-122 permission class. |
| P2 | `golem:agent/host.get-all-agent-types`; `get-agent-type`; `make-agent-id`; `parse-agent-id`; `create-webhook`; `get-config-value` | `durable_host/golem/agent.rs` | Discovery/ID parsing are ungated local metadata; `get-config-value`: **Config / `Read` / exact key**; `create-webhook`: **Agent / operation-specific webhook verb / promise resource** before external creation. |
| P2 | `golem:tool/host.get-all-tools`; `get-tool` | `durable_host/tool/mod.rs` | Ungated tool discovery metadata. |
| P2 | `golem:tool/host.tool-rpc.new`; `invoke`; `async-invoke-and-await`; `invoke-and-await`; future `get`; `cancel`; drops | `durable_host/tool/mod.rs` | Invoke variants: **Tool / `Invoke` / resolved command + arguments** before the invocation-backend handoff. Denials and the current unavailable-backend result are durable. `new`, future result access/cancel, and drops are local/admitted plumbing. Functional dispatch is owned by GOL-35. |
| P2 | `golem:permissions/inspect.inspect-card`; card metadata getters; `derive.derive`; `derive-from-wallet`; `derive-scope`; `revoke.revoke-card`; `wallet.self-wallet`; `self-version`; `install-card`; `kernel-introspection.list-modules`; `validate-grant`; drops | `durable_host/permissions/mod.rs` | **Already protected elsewhere:** existing card-specific authority checks (`CardVerb::Derive`, install/transfer and possession/ancestor revoke rules). Inspection/wallet/version/kernel validation and drops are card metadata/plumbing under those APIs, not silently omitted. |
| P2 | `golem:api/host` agent listing, promise operations, metadata/update/fork/revert/resolve operations | `durable_host/golem/v1x.rs` | Agent-observing/mutating operations: **Agent / matching operation-specific verb / target agent + method/index/invocation/plugin resource**. Legacy `get-agent-metadata` and `resolve-agent-id-strict` denial uses their existing `none` result; operations with result errors use `agent-operation-error.permission-denied`. Self-only oplog markers, idempotence mode/key generation, trap and promise-local plumbing are ungated. |
| P2 | `golem:api/oplog.get-oplog-index`; `set-oplog-index`; get/search iterator `new`; `get-next`; `enrich-oplog-entries`; drops | `durable_host/golem/v1x.rs` | Reads/search: **Oplog / `Read` / exact index or range** before service access. Iterator APIs return typed `oplog-read-error`; legacy enrichment preserves its string error and returns `"permission denied"`. Cursor setters and iterator reads inherit admission; drops are ungated. |
| P2 | `golem:api/retry.get-retry-policies`; `get-retry-policy-by-name`; `resolve-retry-policy`; `set-retry-policy`; `remove-retry-policy` | `durable_host/golem/retry_api.rs` | Ungated worker-local retry policy configuration; no gated external authority. |
| P2 | `golem:api/context` span/context getters/setters/start/finish/header forwarding and drops | `durable_host/golem/invocation_context_api.rs` | Ungated invocation tracing context plumbing. |
| P2 | `golem:durability/durability.observe-function-call`; `begin-custom-durable-invocation`; custom invocation `finish`; `[drop]` | `durable_host/durability.rs` | Ungated durability protocol plumbing; protected semantic operations authorize before durable `Start`. |
| P2 | `golem:schema/wire` conversion/transport functions | `golem-schema/src/schema/wit/mod.rs` | Ungated pure schema/value encoding and handle transport; handle authority remains with quota/secret/card APIs. |

The outbound agent-RPC implementation is in `durable_host/wasm_rpc/mod.rs`; its invocation methods are
**Agent / `Invoke` / target agent owner + resolved method** and are gated at the caller before activation
or dispatch even though the low-level interface is reached through the registered agent host rather than
registered as a separate linker interface. Tool-RPC authorization is owned directly by
`durable_host/tool/mod.rs`; GOL-35 will attach functional dispatch after that boundary.

Secret handles also cross host-to-guest boundaries that are not standalone linker imports. The complete
`Hold` admission set is: secret-backed agent config, live initialization/method input, synchronous and
future outbound-RPC results, tool success/custom-error results, and nested secret handles returned by a
reveal. Each live boundary authorizes the complete recursively discovered target set before durable
completion or guest resource minting. Completed replay remints only previously admitted snapshots and
does not consult current authority.

**Exit criterion:** every registered host import has a row and an explicit disposition.

## Milestone 1 — Define replay and denial durability

### 1.1 Trusted replay

- [x] Add an explicit replay path before permission-target construction.
- [x] During replay:
  - return recorded host results where present;
  - recreate local resources and streams as already admitted;
  - never call `EffectiveSurface::authorize`;
  - never check wall-clock expiration;
  - never enter the authority synchronization boundary.
- [x] Ensure replay-created descriptors, streams, sockets, and pending operations carry reconstructed admission state when they later cross the live frontier.
- [x] Apply recorded card events only to reconstruct the wallet needed at the live frontier.
- [x] Avoid repeatedly deriving intermediate effective surfaces during replay; derive once after reconstruction or snapshot restoration.
- [x] Add a transition hook that synchronizes authority once before accepting the first new live operation.

### 1.2 Durable denials

A live denial is a host-call result, not an authorization event.

- [x] For protected operations already using `CallHandle`, persist the compatible denial through their normal durable response envelope.
- [x] Ensure denial recording does not invoke the backend or mark the operation as admitted.
- [x] For protected operations without a durable response envelope, introduce the smallest operation-specific durable result at the semantic boundary.
- [x] For streams, record admission/denial at stream creation or operation start—not per chunk or poll.
- [x] Do not add a global `PermissionDecision` entry or record successful permission checks separately from the operation.
- [x] Ensure a snapshot contains enough resource/admission state to resume without reauthorization.
- [x] Ensure an incomplete operation whose live `Start` followed authorization remains admitted after recovery.

### 1.3 Guest API errors

- [x] Use existing standard errors:
  - filesystem: `NotPermitted`
  - sockets/DNS: `AccessDenied`
  - HTTP: `HttpRequestDenied`
  - RPC: `RpcError::Denied`
- [x] Reuse existing KV, blobstore, and secret typed errors.
- [x] Change agent `get-config-value` to return a typed result with `PermissionDenied`; keep shared `wasi:config/store.error` compatible and map denial to existing `Upstream("permission denied")`.
- [x] Use typed oplog errors for iterator APIs that previously could not represent denial; keep legacy `enrich-oplog-entries` on its existing string error.
- [x] Preserve legacy optional `get-agent-metadata` and `resolve-agent-id-strict`; map denial to `none` rather than widening either signature.
- [x] Preserve the WebSocket error variant; map connect denial to existing `Other("permission denied")`.
- [x] Preserve plain secret `id`/`metadata`; enforce `Hold` before the host transfers the handle instead of reauthorizing handle inspection.
- [x] Regenerate all affected WIT bindings and update callers.
- [x] Remove the possibility of representing policy denial as `anyhow` or a Wasmtime trap.

**Exit criteria:**

- A live denial followed by restart/replay returns the same non-trapping result without evaluating authority.
- An admitted incomplete call resumes without reauthorization.
- No successful permission check has its own oplog entry.

## Milestone 2 — Add cancellation-proof authority invalidation

The common live path cannot call the current expensive synchronization boundary on every operation.

### 2.1 Per-worker generations

- [x] Add a per-worker atomic `published_authority_generation`.
- [x] Add `processed_authority_generation` to durable worker state.
- [x] Keep the global authority-recovery open/closed gate separate.
- [x] Initialize restored workers as not ready for fast authorization until status/oplog reconciliation completes.

### 2.2 Publisher integration

For every authority-event producer:

- [x] Append and durably commit the event.
- [x] Fold/publish worker status.
- [x] Release-publish the new worker generation from the commit/status actor.
- [x] Only then complete the producer request.

Cover:

- [x] card installation
- [x] revocation
- [x] transfer started
- [x] transfer received
- [x] transfer completion/confirmation
- [x] future direct wallet mutations

Publication must happen inside cancellation-proof actor work, not in caller code after an awaited commit.

### 2.3 Slow-path completion

- [x] Keep `published != processed` for the entire synchronization operation.
- [x] Drain events to quiescence under the existing boundary lock.
- [x] Complete wallet mutation and corresponding terminal oplog records.
- [x] Refresh card interest.
- [x] Recompute invocation-scope state.
- [x] Rederive the effective surface only if wallet/scope contents changed.
- [x] Update the cached expiration deadline.
- [x] Adopt the latest published generation only after all state is coherent.
- [x] If another generation arrives during synchronization, continue draining before reopening the fast path.

### 2.4 Expiration

- [x] Cache the earliest live expiration among wallet cards and invocation-scope roots.
- [x] Fast path compares current time only with that deadline.
- [x] Scan/process expiration only when the deadline is due.
- [x] Publish/process expiration as an authority-state change before returning to the fast path.
- [x] Never use the wall clock for replay reconstruction.

**Exit criteria:**

- A committed event cannot exist without eventually making the generation stale.
- No fast path can observe a partially updated wallet or effective surface.
- One event burst causes one slow synchronization, after which calls return to the fast path.

## Milestone 3 — Implement the live authorization API

### 3.1 Direct context API

Add a live-only API conceptually equivalent to:

```rust
async fn authorize_live_permission(
    &mut self,
    target: &PermissionTarget,
) -> Result<LiveAuthorizationPermit, PermissionDenied>;

async fn authorize_live_permissions(
    &mut self,
    targets: &[PermissionTarget],
) -> Result<LiveAuthorizationPermit, PermissionDenied>;
```

The exact error wrapper may need to preserve executor failures separately from policy denial.

- [x] Assert or encode that these APIs cannot be used during replay.
- [x] Authorize directly against `state.agent_effective_surface`.
- [x] Do not construct `AuthCtx`.
- [x] Do not clone the effective surface.
- [x] Return a lightweight permit that proves one stable snapshot admitted the operation.
- [x] Permit lifetime does not retain the authority lock.

### 3.2 Fast path

Inside one stable state access:

- [x] Verify execution is live and authority state is initialized.
- [x] Verify global authority is open.
- [x] Load published generation.
- [x] Verify `published == processed`.
- [x] Verify expiration is not due.
- [x] Authorize against the cached surface.
- [x] Recheck global-open and generation state.
- [x] If either check changed, discard allow or deny and enter the slow path.
- [x] Return policy denial only from a stable snapshot; a concurrent grant may invalidate a stale denial.

### 3.3 Slow path

- [x] Enter the existing serialized card-event boundary.
- [x] Wait for or recover authority if globally closed.
- [x] Synchronize events and expiration.
- [x] Authorize against the resulting surface while still at the boundary.
- [x] Release the lock before beginning the admitted operation.
- [x] Fail closed if authority cannot be recovered.

### 3.4 P3 Accessor API

- [x] Implement the same algorithm using one short `Accessor::with` window for fast authorization.
- [x] Add an Accessor slow path using existing serialized-access machinery.
- [x] Do not clone state out of the store to authorize.
- [x] Refactor protected `CallHandle::start_access` paths so an authorization permit prevents a second authority synchronization.
- [x] Remove unconditional authority synchronization from generic unprotected P3 calls.

### 3.5 Observability

- [x] Count slow-path refreshes and policy denials.
- [x] Do not render permission targets or emit per-allow logs on the hot path.
- [x] Do not attach resource names, secrets, or other high-cardinality values to metrics.

**Exit criterion:** unchanged live authority requires no status/oplog/service I/O and no async authority mutex.

## Milestone 4 — Centralize typed target construction

Use the existing concrete classes rather than host-specific strings.

### Target mapping

| Class | Owner | Verbs/resources |
|---|---|---|
| Filesystem | agent owner | `Read`, `Write`, `List`, `Stat`, `Delete` + absolute guest path |
| Network | empty owner | `Connect` + normalized host/port |
| Env | agent owner | `Read` + variable name |
| KV | environment owner | `Read`, `Write`, `Delete`, `List` + store/key pattern |
| Blob | environment owner | `Read`, `Write`, `Delete`, `List` + bucket/key pattern |
| Secret | environment owner | `Hold` at host-to-guest handle admission and `Reveal` before plaintext access + canonical secret key path |
| Config | agent owner | `Read` + config key path |
| Oplog | agent owner | `Read` + index range |
| Agent | target agent owner | operation-specific `AgentVerb` + method/index/invocation/plugin resource |
| Card | account owner | existing permission-management targets |
| RDBMS | environment owner | `Query`/`Mutate` + database/schema/table |
| Tool | tool owner | `Invoke` + command/arguments |

### Work items

- [x] Add centralized builders using concrete `ClassPermissionTarget<C>` types.
- [x] Cache monomorphized owner values in worker state where possible.
- [x] Do not re-parse rendered permission strings in host wrappers.
- [x] Reuse existing owned targets by reference before introducing borrowed target types.
- [x] Build all targets for a multi-resource operation before authorizing any of them.

### Normalization

- [x] Filesystem paths are canonical, absolute, guest-visible paths.
- [x] Reject attempts to escape the guest root through `..`, symlinks, or descriptor-relative paths.
- [x] Never expose executor temporary/backing paths to permission matching.
- [x] Normalize DNS/HTTP hostnames consistently in both card parsing and runtime target construction.
- [x] Normalize IPv4 and effective ports.
- [x] Decide how IPv6 is represented; the current host/port grammar does not support colon-containing hosts.
- [x] HTTP maps to the current network model's host/effective port; method and URI path are not permission resources unless the class is deliberately extended.
- [x] Use existing config/secret segment grammars.
- [x] Use exact KV store/key and blob bucket/key grammars.
- [x] Preserve typed oplog ranges rather than rendering them into strings.

**Exit criterion:** no protected wrapper constructs a target with ad hoc formatting.

## Milestone 5 — Key-value enforcement

Files:

- [`eventual.rs`](golem-worker-executor/src/durable_host/keyvalue/eventual.rs)
- [`eventual_batch.rs`](golem-worker-executor/src/durable_host/keyvalue/eventual_batch.rs)
- [`caching.rs`](golem-worker-executor/src/durable_host/keyvalue/caching.rs)

- [x] `get`, `exists`, `get-many` → `KvVerb::Read`.
- [x] `set`, `set-many`, vacancy fill → `KvVerb::Write`.
- [x] `delete`, `delete-many` → `KvVerb::Delete`.
- [x] `keys`/listing → `KvVerb::List` with the exact store/prefix resource.
- [x] Cover caching `get`, `exists`, `set`, `get-or-set`, and `delete`.
- [x] Do not gate handle drops or completed-future reads.
- [x] Preflight every key in mutating batches under one authority snapshot.
- [x] A denied batch item prevents all backend calls.
- [x] Denied reads do not reveal existence.
- [x] Return existing typed KV denial.
- [x] Persist/replay denial through the operation's durable response.

**Exit criterion:** backend call count is zero on denial and one on allow; no partial batch effects.

## Milestone 6 — Blobstore and secrets

### Blobstore

Files:

- [`blobstore/mod.rs`](golem-worker-executor/src/durable_host/blobstore/mod.rs)
- [`container.rs`](golem-worker-executor/src/durable_host/blobstore/container.rs)

- [x] Read/get/has/info → `BlobVerb::Read`.
- [x] List → `BlobVerb::List`.
- [x] Write/create → `BlobVerb::Write`.
- [x] Delete/clear → `BlobVerb::Delete`.
- [x] Copy preflights source `Read` and destination `Write`.
- [x] Move preflights source `Read`/`Delete` and destination `Write`.
- [x] Multi-object deletion preflights all keys.
- [x] Carry admission into outgoing write streams/tasks.
- [x] Do not charge quota or contact storage before authorization.
- [x] Return existing typed blobstore denial.

### Secrets

File: [`secrets/mod.rs`](golem-worker-executor/src/durable_host/secrets/mod.rs)

- [x] Apply `SecretVerb::Hold` at every host-to-guest handle admission/transfer boundary rather than on later handle inspection.
- [x] Recursively preflight all nested handles in config, invocation input, RPC/tool results, and revealed values under one stable authority snapshot.
- [x] Keep admitted `id` and `metadata` access direct and ungated; possession proves prior `Hold` admission.
- [x] Gate `reveal` with `SecretVerb::Reveal` before contacting the service.
- [x] Audit ID/metadata access for existence leakage: an unauthorized handle is never minted, while an admitted handle may expose only its non-plaintext identity/metadata.
- [x] Do not reveal whether a non-admitted secret exists.
- [x] Return existing `secret-error` for reveal denial and the enclosing operation's existing typed/optional error for `Hold` denial.
- [x] Never log the secret key or value on denial.

**Exit criterion:** denied reveal causes no service call; denied `Hold` mints no guest resource and leaks no metadata.

## Milestone 7 — Config, environment, and oplog

### Config

- [x] Change `get-config-value` WIT to a typed result.
- [x] Build `ConfigVerb::Read` target from agent owner and concrete key segments.
- [x] For secret-backed declarations, preflight `ConfigVerb::Read` and `SecretVerb::Hold` together before durable `Start` or handle minting.
- [x] Authorize before reading config or exposing whether the key exists.
- [x] Replay the recorded typed result without authorization.

### Oplog

- [x] Enumerate every guest-visible oplog read/search API.
- [x] Build `OplogVerb::Read` targets using the exact requested index/range.
- [x] Authorize before opening the oplog service or reading entries.
- [x] Change iterator APIs lacking any denial channel to typed results; preserve `enrich-oplog-entries` and its existing string error.
- [x] Ensure denied ranges reveal no entry count or boundary metadata.

### Environment decision and implementation

The original invocation-materialization design was rejected because it introduced a second environment
state into invocation start and snapshot/update/revert handling. Preserve the executor's established
durable environment lifecycle and make the standard P3 import async in the Golem Wasmtime fork instead.

- [x] Remove invocation-start materialization, the invocation-scoped environment cache,
  `AgentInvocationStarted.environment`, and snapshot save/load cache substitution from the design.
- [x] In the isolated `/Users/vigoo/projects/golem/wasmtime-gol122` checkout, mark
  `wasi:cli/environment.get-environment` async and change the generated P3 host trait implementation to
  `async fn`. `cargo fmt --all -- --check`, `cargo check -p wasmtime-wasi --features p3`, and
  `git diff --check` pass there.
- [x] Make P2/P3 `get-environment` one ordinary durable host call that builds the existing enriched
  environment, authorizes/filters every variable from one stable live authority view, and records the
  filtered result with a dedicated append-only payload pair.
- [x] Replay the recorded environment result without live authorization or rebuilding the current
  environment.
- [x] Add focused P2/P3 allow/deny, revocation, and replay tests proving denied variables are absent and
  recorded results remain deterministic.
- [x] Arguments and current directory remain outside `EnvClass`.
- [x] Verify invocation start and snapshot/update/revert carry no GOL-122 environment cache or payload.

**Exit criterion:** config/oplog denial is typed and environment never exposes a denied variable.

## Milestone 8 — Outbound RPC and agent operations

Files:

- [`wasm_rpc/mod.rs`](golem-worker-executor/src/durable_host/wasm_rpc/mod.rs)
- [`golem/agent.rs`](golem-worker-executor/src/durable_host/golem/agent.rs)
- [`golem/v1x.rs`](golem-worker-executor/src/durable_host/golem/v1x.rs)

- [x] Outbound invocation → `AgentVerb::Invoke` with target owner and method.
- [x] Map guest-accessible view/delete/interrupt/resume/fork/revert/cancel/plugin/debug operations to existing `AgentVerb` variants. Update/get-metadata/target-fork/revert, agent enumeration, self metadata, strict agent resolution, and self-fork are gated; focused legacy-host allow/deny coverage passes.
- [x] Authorize after the final target agent and resource are known.
- [x] Authorize before:
  - target activation;
  - scheduling;
  - idempotency-key-backed request creation;
  - durable `Start`;
  - RPC dispatch.
- [x] Return `RpcError::Denied`.
- [x] Carry the permit into asynchronous dispatch.
- [x] Preserve downstream direct-invocation checks as defense in depth.
- [x] Verify caller-side enforcement uses the caller's wallet and invocation scope.

**Exit criterion:** denied outbound calls never reach worker lookup, activation, scheduling, or transport.

## Milestone 9 — P3 filesystem enforcement

File: [`p3/filesystem.rs`](golem-worker-executor/src/durable_host/p3/filesystem.rs)

### Resource metadata

- [x] Associate descriptors with canonical guest-visible paths.
- [x] Preserve path metadata across descriptor duplication and replay reconstruction.
- [x] Associate admitted stream/task state with its path and permit.
- [x] Handle `/` and `.` preopens without authorizing against host backing paths.

### Operations

- [x] File/data reads → `Read`.
- [x] Directory enumeration → `List`.
- [x] Stat and metadata queries → `Stat`.
- [x] Create/open-for-write, write, truncate, set-size/times → `Write`.
- [x] Remove/unlink → `Delete`.
- [x] Open with multiple access flags preflights every required verb.
- [x] Rename preflights source `Delete` and destination `Write`.
- [x] Hard link preflights source `Read` and destination `Write`.
- [x] Symlink preflights the destination path and any source access required by the chosen semantic model.
- [x] Authorize before quota mutation, filesystem calls, piping, or task spawning.
- [x] Authorize once per admitted stream operation, not per chunk/poll.
- [x] Do not gate drop, polling, descriptor flags, or purely local resource inspection.
- [x] Return `NotPermitted`.

### Tests

- [x] Path traversal cannot escape the guest root.
- [x] Two-path operations are atomic with respect to permission preflight.
- [x] Revocation after stream admission does not cancel that stream.
- [x] A new stream after revocation is denied.
- [x] Replay-created streams are treated as previously admitted.

**Exit criterion:** every filesystem effect has a canonical target and no denied operation touches the backing filesystem.

## Milestone 10 — P3 network, DNS, and HTTP

### DNS

- [x] Normalize hostname before authorization.
- [x] Map resolution to the current network `Connect` policy, or add a distinct verb during Milestone 0.
- [x] Authorize before resolver activity.
- [x] Return `AccessDenied`.

### TCP

- [x] Authorize the normalized remote host/port before connect.
- [x] Treat successful connection admission as covering that connection's lifetime.
- [x] Store endpoint/admission metadata on the socket and derived streams.
- [x] Do not reauthorize individual send/receive chunks under the current `Connect` model.
- [x] Do not gate polling or socket drops.
- [x] Implement the Milestone 0 bind/listen decision.

### UDP

- [x] Connected UDP socket: authorize the connected endpoint once.
- [x] Unconnected `send-to`: authorize each new destination as a semantic operation.
- [x] Carry admission into the send task.
- [x] Decide whether repeated sends to the same endpoint reuse admission or represent new operations.
- [x] Return `AccessDenied`.

### HTTP

File: [`p3/http/send.rs`](golem-worker-executor/src/durable_host/p3/http/send.rs)

- [x] Parse and normalize final URI host and effective port.
- [x] Build `NetworkVerb::Connect` target.
- [x] Authorize before:
  - quota charging;
  - pending-transmission state mutation;
  - body/resource consumption;
  - connection-pool activity;
  - durable `Start`;
  - request conversion or dispatch.
- [x] Carry the permit into the transmission task.
- [x] Authorize each redirect destination before dispatching it.
- [x] Return `HttpRequestDenied`.
- [x] Do not include method/path in the target unless the permission class is intentionally extended.

**Exit criterion:** denied network/HTTP operations produce no DNS, socket, pool, quota, or transmission activity.

## Milestone 11 — Remaining classes and complete audit

### Permission cards

- [x] Audit existing [`permissions/mod.rs`](golem-worker-executor/src/durable_host/permissions/mod.rs) checks against the new live boundary.
- [x] Reuse the shared helper where it removes duplicated synchronization.
- [x] Do not rewrite already-correct card algebra or lifecycle behavior.

### RDBMS

- [x] Determine tables touched by each statement.
- [x] Map read-only statements to `Query`.
- [x] Map mutations/DDL to `Mutate`.
- [x] Preflight every referenced table.
- [x] Reject statements whose resource set cannot be determined safely.
- [x] Cover PostgreSQL, MySQL, and Ignite consistently.
- [x] Authorize before connection use, statement preparation, transaction mutation, or quota charging.

### Tools

- [x] Locate the registered `golem:tool/host.tool-rpc` authorization boundary.
- [x] Build exact `ToolVerb::Invoke` targets from the resolved command path and canonical arguments.
- [x] Authorize before the invocation-backend handoff (and therefore before future entity/RPC/task creation in GOL-35).
- [x] Return a typed `RpcError::Denied` and persist it through the invocation's durable response.
- [x] Cover `invoke`, `async-invoke-and-await`, and `invoke-and-await` with durable typed-denial tests; verify a grant passes authorization and reaches the current unavailable-backend result rather than `Denied`.
- [x] Behavioral denial test resolves a bound tool, performs exactly one authority check, returns typed `RpcError::Denied`, and confirms no tool worker is activated (`tmp/gol122-denied-tool-invocation.log`).
- [x] Keep functional dispatch out of GOL-122; it is owned by GOL-35.

### Final linker audit

- [x] Revisit every P2/P3 linker registration.
- [x] Confirm every protected import has an enforcement test.
- [x] Confirm every intentionally ungated import has a reason in the matrix.
- [x] Confirm no alternate linker path bypasses wrappers.

## Milestone 12 — Test suite

### Permission algebra and target tests

- [x] Owner and resource matching for every enforced class.
- [x] Lower OR semantics.
- [x] Upper AND semantics.
- [x] Negative grants.
- [x] Invocation-scope narrowing.
- [x] Wildcards, path globs, ranges, and port ranges.
- [x] Filesystem and network normalization.
- [x] Multi-target all-or-nothing authorization.

### Authority-boundary tests

- [x] Unchanged generation uses the no-I/O fast path.
- [x] Event generation forces exactly one slow refresh.
- [x] Concurrent generation change invalidates an in-progress allow.
- [x] Concurrent installation invalidates an in-progress deny.
- [x] Event publication survives producer cancellation.
- [x] Closed authority never allows.
- [x] Expiration is visible at the first due live boundary.
- [x] Revocation after admission does not cancel the admitted operation.
- [x] Replay never invokes the authorization helper.

### Wrapper tests

For each family:

- [x] allow calls backend exactly once
- [x] deny calls backend zero times
- [x] denial uses the operation's compatible typed, string, or optional channel; no trap
- [x] no quota/resource/task mutation before allow
- [x] no unauthorized existence leakage
- [x] admitted task retains permit
- [x] new task after revocation is denied

Closure evidence combines family-specific wrappers with the shared authority-boundary tests rather than
duplicating the same generation/revocation test for every class. Every protected import has a compatible
non-trapping denial probe, so a trap fails the test. Countable backends verify one TCP connection, one secret revision
lookup, and zero denied TCP connections, secret lookups, tool activations, or filesystem effects. KV and
blob multi-item mutations verify that one denied target leaves every allowed target untouched. Environment,
config, secret, filesystem, and owner-isolation probes verify absence/no-existence-leak behavior. Filesystem
streams, TCP connections, RDBMS transactions, cache vacancies, and blob read streams verify inherited
admission; the latter two are explicitly suspended, revoked, resumed successfully, then followed by denied
new work. Synchronous no-task families have no admitted resource to retain, and established TCP/WebSocket
I/O is intentionally ungated after connection admission. Shared generation tests plus per-family default-deny
probes cover post-revocation new operations without repeating the same authority transition in every wrapper.

### Replay and recovery tests

- [x] Live denial replays through the same compatible host-result channel after wallet changes.
- [x] Successful operation replays without permission evaluation.
- [x] Snapshot restore treats reconstructed resources as admitted.
- [x] Incomplete admitted operation completes without reauthorization.
- [x] First new operation after replay-to-live synchronizes and enforces current authority.
- [x] Replay does not consult current time or live card services.

### Integration tests

Extend:

- [`scope_cards.rs`](golem-worker-executor/tests/scope_cards.rs)
- [`permissions.rs`](integration-tests/tests/permissions.rs)

Cover:

- [x] allow/deny for every confirmed host-facing class
- [x] invocation scope narrowing
- [x] revocation between operations
- [x] expiration between operations
- [x] owner isolation
- [x] recipient/holder isolation through effective-surface derivation
- [x] suspend/resume
- [x] crash/replay
- [x] snapshot/recovery
- [x] concurrent P3 operations

## Milestone 13 — Performance validation

### Structural requirements

- [x] Replay constructs no permission targets and performs no checks.
- [x] Stable live call performs no status/oplog/service I/O.
- [x] Stable live call acquires no async authority mutex.
- [x] P3 stable call uses one short Accessor state window.
- [x] Effective surface is borrowed, not cloned.
- [x] `AuthCtx` is not created.
- [x] Existing resource handles reuse normalized metadata.
- [x] Streams authorize once per semantic operation, not per chunk/poll.
- [x] Batch operations cross one authority snapshot.
- [x] Successful authorization emits no logs or rendered targets.

### Benchmarks

- [x] Baseline representative host wrappers before enforcement.
- [x] Stable allow and stable deny.
- [x] Slow path with one event and an event burst.
- [x] Wallets with small, medium, and large grant counts.
- [x] Single-key and batch KV.
- [x] Filesystem open and stream creation.
- [x] TCP connect and HTTP dispatch wrapper overhead.
- [x] Record p50/p95 and allocation counts.
- [x] Benchmark before introducing borrowed request types or class indexing.
- [x] Add class-indexed grant surfaces only if algebra scanning remains material after synchronization overhead is removed. The measured matcher does not justify an index.

Post-fix distribution evidence (`tmp/gol122-authorization-bench.log`):

```text
stable TCP allow (64 grants): p50 1.292µs, p95 1.750µs, 0 allocations
stable TCP deny  (64 grants): p50 1.167µs, p95 1.667µs, 0 allocations
filesystem open:              p50 41ns,    p95 42ns,    0 allocations
KV single key:                p50 41ns,    p95 42ns,    0 allocations
one-generation refresh:       p50 1.167µs, p95 1.709µs, 0 allocations
eight-event refresh burst:    p50 1.208µs, p95 1.834µs, 0 allocations
```

**Exit criterion:** measured hot-path cost consists only of state validation, target matching, and the existing short store-access window.

## Recommended implementation sequence

1. Milestone 0: freeze scope and matrix.
2. Milestone 1: replay/denial contracts and WIT changes.
3. Milestones 2–3: generation fast path and authorization permit.
4. Milestone 4: typed target construction.
5. Milestones 5–8: non-P3 service-backed imports.
6. Milestone 7 environment decision.
7. Milestone 9: filesystem.
8. Milestone 10: DNS/sockets/HTTP.
9. Milestone 11: RDBMS/tools decision and final import audit.
10. Milestones 12–13: integration, recovery, and performance validation.

## Definition of done

GOL-122 host-call enforcement is complete when:

- every protected live host operation is authorized against one stable effective surface;
- replay performs no permission checks;
- all live denials replay through their compatible typed, string, or optional host result;
- denied calls create no external or local effect;
- multi-resource operations are fully preflighted;
- revocation/expiration is visible at the next new live semantic operation;
- previously admitted operations survive later revocation;
- every registered import is either tested as protected or explicitly classified as ungated;
- the unchanged-authority hot path has no I/O or async authority lock;
- targeted executor and integration tests, formatting, and lint checks pass.
