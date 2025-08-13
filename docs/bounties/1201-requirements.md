# Issue #1201 — Requirements Understanding (from schema.golem.cloud/app/md/details.md)

Source spec: /Users/fahadkiani/Desktop/development/golem-cli-main/schema.golem.cloud/app/md/details.md

## Problem statement
Enable Golem workers to interact with external gRPC and OpenAPI services in a type-safe way analogous to worker-to-worker WIT-based RPC, but stateless and backed by dynamic stubs inside `worker-executor` with durability.

## Inputs and outputs
- Inputs
  - Protobuf v3 (proto3) with gRPC service definitions
  - OpenAPI 3.0.x (YAML/JSON)
- Outputs
  - WIT packages (package name + version)
  - WIT records/interfaces/functions for services, messages, schemas

## Package and version mapping
- gRPC: package name ← proto `package`; version ← configuration input
- OpenAPI: package name ← sanitized `info.title`; version ← `info.version`

## Type/system mappings
- Protobuf → WIT
  - message → record
  - oneof → variant
  - service → interface; RPC method → function
  - field types (string→string, int32→s32, int64→s64, uint32→u32, uint64→u64, float→float32, double→float64, bool→bool, repeated T→list<T>, optional T→option<T>)
  - Preserve numeric field order in the linear order of WIT fields
  - Nested messages become separate records
  - Functions return `result<response-type, error>`
- OpenAPI → WIT
  - components.schemas → records; required → non-optional
  - Inline schemas → named records (deterministic synthesis)
  - Path + method grouping → collection/resource interfaces with domain-appropriate request/response types
  - Headers mapping:
    - Authorization → auth (request)
    - ETag → version (response)
    - If-Match → expected-version (request)
    - Last-Modified → last-updated (response)
    - All others → optional kebab-case fields

## Naming and determinism
- kebab-case identifiers
- Reserved words prefixed with `%`
- Deterministic name synthesis for inline/anonymous schemas:
  - Request bodies: `{path}-{method}-request-body`
  - Response bodies: `{path}-{method}-response-body`
  - Array items: `{parent-type}-item`
  - Nested objects: `{parent-type}-{field-name}`
  - Parameters: `{path}-{method}-params`
- Collision handling: suffixing (`_record`, `_params`, `_result`), otherwise fail with actionable diagnostics
- Valid WIT identifiers; ≤ 64 characters

## Error and auth models
- Error variant (minimum expressiveness):
  - unauthorized, not-found, validation-error { fields: list<string> }, rate-limited { retry-after: u32 }, server-error { message: string }
- Auth records: bearer-auth (token, scheme), basic-auth (username, password), api-key-auth (key)

## Runtime integration (worker-executor)
- Dynamic stubs added at link-time for all generated WITs (gRPC + HTTP)
- Stubs perform external calls with appropriate auth/headers/transforms
- Durability:
  - Interact with record/playback mode and oplog per call (persist request/response)
  - Ensure replay correctness and retry semantics where appropriate

## CLI/manifest/storage
- Extend `golem.yaml` to allow deps of type `grpc` and `openapi`
- `golem-cli`:
  - Capture and canonicalize schemas
  - Store structured representations needed by executor
  - Update component create/update REST APIs to carry new deps/metadata
  - Provide a `generate-wit` preview

## Testing matrix
- Unit
  - Generators (type mapping, naming, collision, header/auth transforms)
- Integration
  - CLI + generators; manifest parsing; REST API flow (mocked)
- System/E2E
  - OpenAPI: Cloudflare, OpenAI, GitHub (plus surprise schemas)
  - gRPC: grpcb.in, Google TTS (plus surprise services)
  - Validate generated WIT; invoke real endpoints through dynamic stubs; verify durability

## Acceptance criteria alignment
- High-quality Rust, modular, DRY, strongly typed
- End-to-end tests pass against real services; CI green
- Up-to-date with head branch
- Well-documented for developers and end-users

## Open design questions to track
- gRPC streaming (server/client/bidi) strategy in v1 (defer vs async streams)
- OpenAPI polymorphism (oneOf/anyOf/allOf) and discriminators mapping
- Enum mapping (OpenAPI and proto) to WIT variants vs strings
- Resource grouping heuristics (by base path vs tags) and pagination conventions

## Links
- Org: https://github.com/golemcloud
- Bounty: https://github.com/golemcloud/golem/issues/1201 