# Rust tool definition macros (golemcloud/golem#3532) — implementation plan

Implements the Rust `#[tool_definition]` authoring surface from the agent-tools
spec (§5.1, §5.2.1, §5.3.1 grep example, §5.3.5.1 git subset, §5.8.1
implicit-body convention), testable end-to-end through the exported
`golem:tool/guest@0.1.0` `discover-tools` function.

Everything in this list ships in a **single PR** — including the cross-cutting
`golem-schema` changes, subtree support, the git/Remote multi-trait example, and
the §4.7 cross-language equivalence anchor. Nothing the ticket describes is
deferred.

## Dependency

This work sits on top of the `value-type-refactoring-6` branch (currently in
`../golem-2`, to be merged onto this branch). That branch adds the
standard-Rust-type → rich-schema-node mappings this plan relies on:

- `std::path::PathBuf` → `SchemaType::Path { direction: InOut, kind: Any }`
- `url::Url` → `SchemaType::Url`
- `chrono::DateTime` → `SchemaType::Datetime`
- `std::time::Duration` → `SchemaType::Duration`
- `HashMap`/`BTreeMap<K,V>` → `SchemaType::Map`
- new `SchemaType::{text, path, url, quantity, secret}` constructors

## Key context (verified against the repo)

- The repo WIT differs from the spec: the spec's separate `type-tree` /
  `value-tree` and `*-c` constraint records are gone. Types and values are
  delegated to `golem:core/types@2.0.0`'s schema-graph model. See
  `sdks/rust/golem-rust/wit/deps/golem-tool/common.wit` and `guest.wit`.
- `tool` record is `{ version, commands: command-tree, schema: schema-graph }`.
  Command bodies reference types by `type-node-index` into the shared
  `tool.schema` pool. `schema.root` is a structurally-required placeholder, not
  the semantic root.
- The command tree, `option-shape`, `flag-shape`, `constraint`
  (`mutex-groups`/`implies`/`all-or-none`/`requires-all`/`requires-any`/`forbids`),
  `stream-spec`, `result-spec`, `error-case`, `command-annotations` all still
  exist in the tool WIT.
- The schema model (`golem-schema/src/schema/schema_type.rs`) carries constraints
  on rich semantic types via inline sidecars (`Text{TextRestrictions}`,
  `Url{UrlRestrictions}`, `Path{PathSpec}`, `Quantity{QuantitySpec}`,
  `Binary{BinaryRestrictions}`). Numeric variants `S8..U64, F32, F64` currently
  carry only `MetadataEnvelope`. `SchemaType` derives `Eq`.
- The `golem-agentic` world already exports `golem:tool/guest@0.1.0`; the current
  guest impl `sdks/rust/golem-rust/src/agentic/tool_impl.rs` is a placeholder
  (`discover_tools` returns empty, `get_tool`/`invoke` report unknown).
- Pattern to mirror (agents):
  - definition macro `sdks/rust/golem-rust-macro/src/agentic/agent_definition_impl.rs`
    inserts a `__register_agent_type()` metadata producer into the trait.
  - implementation macro `sdks/rust/golem-rust-macro/src/agentic/agent_implementation_impl.rs`
    emits the `#[ctor]` that calls `<Impl as Trait>::__register_agent_type()`.
  - registry `sdks/rust/golem-rust/src/agentic/agent_registry.rs` (global `State`).
  - builder `sdks/rust/golem-rust/src/agentic/extended_agent_type.rs`
    (`ExtendedAgentType` → WIT record, merging per-method schema graphs).
  - tests `sdks/rust/golem-rust/tests/agent.rs` call `Impl::__register_agent_type()`
    directly, then query the registry.

## Locked decisions

### D1 — Registration: definition builds metadata, implementation registers

- `#[tool_definition]` only generates the metadata producer (a `where Self: Sized`
  static fn, e.g. `__register_tool()`, plus a hidden `ToolDefinitionDescriptor`)
  and performs compile-time validation. It does **not** register.
- `#[tool_implementation]` emits the `#[ctor]` that calls
  `<Impl as Trait>::__register_tool()`. Because the ctor only exists on
  implemented types, a tool with no implementation is never registered — a
  registered tool is necessarily implemented in that wasm (operator invariant).
- A **minimal `#[tool_implementation]`** (registration trigger only; real
  `invoke` dispatch is a downstream ticket) is in scope so the macro is testable
  through `discover-tools`.
- Tests register by calling `Leaf*::__register_tool()` directly (like the agent
  tests), then assert via the `discover_tools()` guest export.

### D2 — Numeric constraints: `Option<NumericRestrictions>` (one PR)

- Add `restrictions: Option<NumericRestrictions>` to all 10 numeric `SchemaType`
  variants (`S8..U64, F32, F64`). `Option<>` (not a non-optional empty-by-default
  struct) is chosen to minimize hot-path cost: `SchemaType`/`SchemaValue` are
  passed, serialized, and converted on the hot path, and the common
  unconstrained case is a single `None` tag rather than three carried inner
  `Option`s.
- `NumericRestrictions { min: Option<NumericBound>, max: Option<NumericBound>,
  unit: Option<String> }`.
- `NumericBound` is `Eq`-safe and covers the full range of every numeric repr
  (`SchemaType` derives `Eq`, and `i64`-mantissa cannot represent `u64::MAX`):
  `enum NumericBound { Signed(i64), Unsigned(u64), Float(u64 /* canonical bits */) }`.
  Floats are stored as canonical bits, rejecting `NaN`/`inf` and normalizing
  `-0.0`.
- **Canonicalization invariant** (closes the `None` vs `Some(empty)` hazard the
  `Option<>` form introduces, keeping derived `Eq` and §4.7 byte-equivalence
  intact):
  - `NumericRestrictions::is_empty()` ⇔ `min.is_none() && max.is_none() &&
    unit` is `None`/empty.
  - Smart constructors and decoders always collapse empty → `None`; `unit:
    Some("")` normalizes to `None`.
  - Well-formedness forbids `Some(empty)` (it must never be constructible).
  - `#[serde(default, skip_serializing_if = "Option::is_none")]` so the wire form
    for unconstrained numerics is unchanged.

### D3 — Rich types: standard types for identity, `#[arg]` for refinement

- Type identity comes from standard Rust types via the dependency branch
  (`PathBuf`/`url::Url`/`chrono::DateTime`/`Duration`/`BTreeMap`). No new SDK
  wrapper types.
- The macro does `#[arg]`-driven **node refinement** on the schema node returned
  by `<Ty as Schema>::get_type()`:
  - `regex` / `min_length` / `max_length` → `Text { TextRestrictions }`
  - `direction` / `kind` / `mime` / `accepts_stdio` → `Path { PathSpec }` (+ the
    tool-model positional `accepts-stdio` flag, see D5)
  - url `schemes` → `Url { UrlRestrictions }`
  - numeric `min` / `max` / `bounds` / `unit` → numeric `NumericRestrictions`

### D4 — Key-value parameters use `Map`

- Key-value data maps to `SchemaType::Map` / `SchemaValue::Map`, never
  `list<tuple>`.
- Author git's `config` as `BTreeMap<String, String>` (deterministic key order,
  required for the §4.7 canonical anchor; `HashMap` order is nondeterministic).
- git's `config` is also `repeatable = "repeated"` (`-c a=1 -c b=2`). The tool
  WIT `repeatable-shape` carries a single element `%type` while the logical
  collected value is a `Map`. Model the repeatable element's value type via
  `repeatable-shape` and collect into a `Map<string, V>` node; the exact
  element-vs-map shaping is finalized in Phase 0.

### D5 — `accepts_stdio` is added to the tool WIT (not deferred)

- `accepts_stdio` (grep's `files = "tail", accepts_stdio = true`) is not in the
  current WIT. Add `accepts-stdio: bool` to the tool-model `positional` and
  `tail-positional` records in `golem:tool` common.wit (tool-specific home,
  smaller blast radius than the shared schema model). Synced to all SDK WIT
  copies via `cargo make wit`.

### D6 — Cross-trait subtree assembly via runtime descriptor

- A proc macro expanding `Git` cannot read the metadata generated by the `Remote`
  macro, so the command tree for `git → remote → add` cannot be assembled at
  macro-expansion time. Instead:
  - Each `#[tool_definition]` emits a hidden `ToolDefinitionDescriptor`
    (`fn metadata() -> ExtendedToolType`).
  - The parent declares subtrees explicitly with
    `#[command(subtree = path::to::Remote)]` (bare `-> Remote` is not a safe macro
    target).
  - The parent's generated `__register_tool()` calls the child descriptor's
    `metadata()` at registration time, turns the parent method into a command
    node with `body = none`, and grafts the child root's subcommands beneath it.
  - The child root name must match (or be explicitly overridden against) the
    parent command name; validated at registration.
  - A subtree-only trait is not registered as a top-level tool unless it has its
    own `#[tool_implementation]`.

### D7 — No command-body input-record in metadata

- The WIT `command-body` lists positionals/options/flags individually by
  `type-node-index`; there is no single input-record type node. The invocation
  record (`guest.invoke`'s `typed-schema-value`) is a derived view.
- Lock a canonical invocation-record field order, used by the derived record,
  validation, help rendering, and future dispatch/client codegen:
  1. inherited globals from root → parent (at each node: options, then flags),
  2. fixed positionals (declaration order),
  3. tail positional (single list-typed field),
  4. body options,
  5. body flags.

### D8 — Globals stored once

- Globals are stored only on the `command-node` where declared (recursive "this
  level and downward" semantics). They are **not** cloned into descendant
  `command-body` options/flags. An effective-globals view is derived where
  needed. A validator checks body-local names are unique against inherited
  globals.

### D9 — Defaults and `value_is` literals

- `positional.default`, `option-spec.default`, and `value-is` are
  `schema-value-tree`s interpreted against the referenced type node. The macro
  layer parses Rust attribute literals, resolves the referenced argument's
  `SchemaType`, produces a `SchemaValue`, and encodes via the schema model's
  value encoder. Validated against the referenced type node at registration.
- Enum-default case names need a canonical casing rule (tool-facing) so the §4.7
  anchor is stable.

### D10 — Error metadata via a derive

- `Result<T, E>` does not let the tool macro recover `#[error(kind, exit_code)]`
  from `E`. Add `#[derive(ToolError)]` with a `#[tool_error(kind = "...",
  exit_code = ...)]` helper attribute (an inert attribute alone cannot drive
  enum-variant metadata). Tool methods returning `Result<T, E>` require
  `E: ToolErrorSchema` (+ `Schema`), and the macro reads error-case metadata from
  that.

## Phases

### Phase 0 — Design locks (before coding)

Finalize and write down, with golden examples:

- Subtree mechanism: `ToolDefinitionDescriptor` shape, `#[command(subtree=…)]`
  syntax, child-root-name matching/override rule, runtime graft algorithm (D6).
- Canonical invocation-record field order (D7).
- Defaults / `value_is` literal → `SchemaValue` conversion strategy and enum-case
  canonical casing (D9).
- Error metadata strategy: `ToolError` derive + helper attribute (D10).
- Numeric: `Option<NumericRestrictions>`, `NumericBound`, normalization invariant
  (D2).
- `accepts-stdio` WIT addition (D5); key-value/repeatable `Map` shaping (D4).
- Canonical casing rules for tool / command / arg names.

### Phase 1 — Schema + WIT model changes

`golem-schema`:

- Add `Option<NumericRestrictions>` to `S8..U64, F32, F64`
  (`skip_serializing_if = Option::is_none` + empty-normalization to `None`);
  add `NumericBound`, `NumericRestrictions`, smart constructors.
- Update the ~120 numeric match sites:
  - `validation/subtyping.rs`: numeric narrowing within the same repr
    (`sub.min >= sup.min`, `sub.max <= sup.max`, `None` = unbounded; inclusive),
    equivalence compares normalized restrictions exactly.
  - `validation/value.rs`: numeric range/unit checks.
  - `validation/well_formedness.rs`: `min <= max`, bounds fit the repr, integer
    variants reject fractional bounds, float bounds reject `NaN`/`inf` and
    normalize `-0.0`, reject `Some(empty)`.
  - `protobuf.rs` + the schema `.proto`: numeric message fields.
  - `wit/encode.rs` + `wit/decode.rs` + `golem-core-v2.wit`: numeric variant
    cases gain a payload (payloadless → payload-carrying = versioned wire
    evolution; decoders normalize missing → `None`).
  - `proptest_strategies.rs`, `conversion.rs` (primitives still emit `None`).

`golem:tool` common.wit:

- Add `accepts-stdio: bool` to `positional` and `tail-positional`.

Propagation:

- Sync WIT/protobuf to the TS / Scala / MoonBit encoders/decoders so byte
  equivalence holds; run `cargo make wit`.
- Golden cross-SDK vectors: bare `u32`, `u32 min=1`, `u32 bounds=(0,100)`,
  `s64 bounds=(0, i64::MAX)`, `u64` near `u64::MAX`, `f64 min=0.0`, each `+unit`,
  and empty → `None` roundtrip.

> **Phase 1 status (oracle-reviewed):** the Rust core (golem-schema +
> golem-common + protobuf + golem-core-v2/tool WIT + golem-schema-derive) is
> complete, verified, and oracle-confirmed correct (4 bug fixes accepted).
> **Deferred-within-PR gate:** the TS / Scala / MoonBit schema-model codec sync
> for the new numeric WIT/proto/tool shapes is intentionally **sequenced after
> the Rust phases** but is **required same-PR work that must land before Phase 6
> / final merge** (oracle SHOULD-FIX 2). Tracked in Phase 6.

### Phase 2 — golem-rust runtime

> **Phase 2 status (oracle-reviewed, CLEAR):** the golem-rust runtime mirror is
> complete and oracle-confirmed. Delivered in
> `sdks/rust/golem-rust/src/agentic/`: `tool_registry.rs` (deterministic
> root-name-keyed registry), `extended_tool_type.rs` (runtime mirror +
> graph-merge `to_tool()`, full validator mirroring `golem-common`'s
> `validate_tool`, help/argument-help renderer, subtree graft),
> `tool_refinement.rs` (D3 node refinements), `errors.rs` (`ToolError`), and
> registry-wired `tool_impl.rs` discovery/lookup. Oracle blockers resolved:
> subtree index-remap/dispatcher/annotation semantics fixed; panic-safe
> validator + helpers; and per-argument schema graphs are now explicitly
> checked for dangling refs (`check_graph_closed`, mirroring canonical
> `check_type_refs`/`check_def_refs`), which makes per-arg default/`value-is`
> validation equivalent to validating against the merged tool schema. SDK
> build/clippy/tests green (lib 60 passed/4 ignored, agent 29 passed). `invoke`
> remains a placeholder pending the later phase.

- `tool_registry.rs`: global `State` with `register_tool`, `get_all_tools`,
  `get_tool_by_name` (mirror `agent_registry.rs`).
- `extended_tool_type.rs`: Rust mirror of the WIT `tool` / `command-tree` /
  `command-node` / `command-body` / specs, with `to_tool()` merging per-arg
  schema graphs into one `tool.schema` (reuse the agent graph-merge/encode
  pattern). **No synthetic input-record node in metadata.** Add:
  - `EffectiveCommandBody`, `effective_globals(command_index)`,
    `canonical_input_fields(command_index)` (D7),
  - `encode_schema_value_default(...)` (D9),
  - validators: name uniqueness vs inherited globals (D8), default / `value_is`
    type-match (D9), all `type-node-index` references resolve.
- Node-refinement helpers (D3): regex/min/max-len → `Text`; direction / kind /
  mime / `accepts_stdio` → `Path` (+ positional `accepts-stdio`); schemes →
  `Url`; min/max/unit → numeric.
- `ToolError` runtime surface; help-text renderer (a `Tool` + command-path →
  formatted string at any depth: root, named subcommand, individual argument).
- `ToolDefinitionDescriptor` trait + subtree-graft runtime merge (D6).
- Wire `tool_impl.rs` `discover_tools` / `get_tool` to the registry.

### Phase 3 — Macro: attribute parsing (golem-rust-macro)

- New `tool/` module; register in `lib.rs`: `#[tool_definition]`, minimal
  `#[tool_implementation]`, `#[derive(ToolError)]`, and inert
  `#[arg]` / `#[command]` / `#[constraint]` / `#[result]`.
- Parsers:
  - arg kind: `positional` | `option` | `flag` | `tail` | `global`.
  - option/flag shapes: `repeatable = repeated|delimited|either` (+ `delim`),
    `count-flag` (+ `max`), `optional-scalar`, `negatable`, `default`, `short`,
    `aliases`, `env`, `required`.
  - refinements: `regex`, path `kind` / `direction` / `accepts_stdio`, url
    `schemes`, numeric `min` / `max` / `bounds` / `unit`.
  - `#[command(subtree = path, aliases = [...], annotations(destructive,
    read_only, idempotent, open_world))]`.
  - `#[constraint(...)]`: `mutex_groups`, `all_or_none`, `implies`,
    `requires_all`, `requires_any`, `forbids`, `value_is`.
  - `#[result(formatters = [...], default = "...")]`.
  - `#[tool_error(kind, exit_code)]` (on `ToolError` enum variants).
  - doc parsing: `///` on trait/method → `doc.summary` / `doc.description`;
    `#[arg(doc = "...")]` for params; examples.

### Phase 4 — Macro: metadata synthesis

- Trait name → tool name (kebab). Implicit-body detection: the method whose
  snake_case name equals the tool's snake_case name is the root command body.
  Enforce `commands[0].name == tool.name`; **compile error on divergence**
  (§5.8.1).
- Parameter projection: `bool` → flag; `Vec<T>` at tail → tail positional;
  `Vec<T>` elsewhere → repeatable option; `Option<T>` → not-required; primitive
  → positional; struct → options-object; `BTreeMap`/`HashMap` → `Map`;
  `#[command(subtree=…)]` method → `body = none` node grafted at runtime;
  `"global"` args propagate to this command + descendants (stored once, D8).
- Per typed arg: `<Ty as Schema>::get_type()` then apply `#[arg]` refinement;
  merge into `tool.schema`. Build the flattened command tree (root at index 0),
  globals, options/flags/positionals/streams/result/errors/annotations/
  constraints.
- Emit `__register_tool()` building `ExtendedToolType` (+ descriptor);
  `#[tool_implementation]` emits the `#[ctor]` calling `<Impl>::__register_tool()`.

### Phase 5 — Tests (through discover-tools)

- Port **grep** (§5.3.1) — single trait — into `golem-rust/tests/tool.rs`
  (feature `export_golem_agentic`): register via `LeafGrep::__register_tool()`,
  call `discover_tools()`, assert full metadata.
- Port **git + Remote subtree** (§5.3.5.1): multi-level subcommands, pure
  dispatchers, aliases, per-command annotations, tail separator, optional
  trailing positional, option aliases/env/default, count-flag, negatable flag,
  all repeatable modes, several constraint kinds, multi-level globals
  propagation, `Url`/`Datetime`/`Map`/`Enum` nodes, multiple result formatters,
  mixed usage/runtime exit codes.
- Targeted tests: canonical invocation-record field order (D7); default /
  `value_is` schema-value-tree encoding (D9); globals declared once but effective
  in descendants (D8); numeric bounds incl. `u64::MAX` (D2);
  no-registration-without-`#[tool_implementation]` (D1); help-text rendering at
  root / subcommand / argument depth.
- `trybuild` compile-fail: body/name divergence (§5.8.1), bad `#[arg]`, invalid
  subtree return/descriptor shape.

### Phase 6 — §4.7 cross-language equivalence anchor

- Ensure canonical ordering and casing throughout the produced metadata.
- Snapshot the canonical grep/git `Tool` metadata as the reference the
  cross-language verification ticket (#3560) reuses across SDKs.
- **Deferred-within-PR gate (oracle SHOULD-FIX 2):** complete the TS / Scala /
  MoonBit schema-model codec sync for the new numeric WIT/proto/tool shapes
  (`numeric-restrictions`, `numeric-bound`, `accepts-stdio`, the `option-shape`
  list/map split, `duplicate-key-policy`). Regenerate generated bindings, update
  the hand-written codecs, and prove byte-equivalence against the golden
  vectors. This is required before final completion of this PR.

### Phase 7 — Verify

- `cargo test -p golem-schema`
- `cargo test -p golem-rust --features export_golem_agentic`
- `cargo build -p golem-rust-macro`
- `cargo make wit` (WIT source changed)
- `cargo fmt` + `cargo clippy` per `sdks/rust/AGENTS.md` (the SDK is outside
  `cargo make build`; do not hand-edit `wit/deps`, regenerate instead).

## Phase 0 — Finalized design (grounded against the repo)

This section turns D1–D10 into concrete, code-level artifacts the later phases
copy verbatim. Every claim below was verified against the current tree on branch
`tools-rust-macro-1` (head `2def062bb Value type refactoring 6 (#3662)`).

### F0.1 — Grounded file inventory (numeric change, Phase 1)

Numeric variants `S8..U64, F32, F64` are payloadless today. They are matched in
exactly these files (verified):

- `golem-schema/src/schema/schema_type.rs` — variant defs, `metadata()` +
  `metadata_mut()` accessors, 10 bare constructors.
- `golem-schema/src/schema/wit/encode.rs` (~L362–372), `wit/decode.rs`
  (L297–306) — `wire::SchemaTypeBody::{S8Type..F64Type}`.
- `golem-schema/src/schema/protobuf.rs` (encode L212–221, decode L293–302) —
  `Body::{S8Type..F64Type}(ProtoEmpty{})`.
- `golem-schema/src/schema/validation/{subtyping,value,well_formedness}.rs`.
- `golem-schema/src/schema/proptest_strategies.rs`, `conversion*.rs`.

Wire types come from `wit_bindgen::generate!` over
`golem-schema/wit/deps/golem-core-v2/golem-core-v2.wit` (`schema-type-body`
variant, L218). Proto is `golem-api-grpc/proto/golem/schema/schema.proto`
(`SchemaType.body` oneof, fields 4–13 `golem.common.Empty`). `golem-core-v2` and
the tool WIT are brand-new this cycle (unreleased), so evolving the payloadless
numeric cases is a free versioned change, not a compatibility break.

### F0.2 — Numeric model (D2), exact Rust

```rust
/// Bound usable across every numeric repr. `SchemaType` derives `Eq`, and an
/// `i64` mantissa cannot represent `u64::MAX`, so the bound is a 3-family sum.
/// Floats store canonical IEEE-754 bits (NaN/inf rejected at construction,
/// `-0.0` normalized to `+0.0`) to stay `Eq`-safe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema, ...)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum NumericBound {
    Signed(i64),
    Unsigned(u64),
    FloatBits(u64),
}
impl NumericBound {
    pub fn float(v: f64) -> Result<Self, NumericBoundError>; // reject NaN/inf, normalize -0.0
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema, ...)]
#[serde(rename_all = "camelCase")]
pub struct NumericRestrictions {
    #[serde(default, skip_serializing_if = "Option::is_none")] pub min:  Option<NumericBound>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub max:  Option<NumericBound>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub unit: Option<String>,
}
impl NumericRestrictions {
    pub fn is_empty(&self) -> bool;            // min/max none && unit none-or-""
    pub fn normalize(self) -> Option<Self>;    // empty -> None; unit Some("") -> None
}
```

Each numeric variant gains the field (bare constructors keep `restrictions: None`):

```rust
S8 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restrictions: Option<NumericRestrictions>,
    #[serde(default, skip_serializing_if = "MetadataEnvelope::is_empty")]
    metadata: MetadataEnvelope,
},
```

Accessor match arms become `SchemaType::S8 { metadata, .. } | ...`. The macro
sets restrictions by building the struct literal with
`restrictions: NumericRestrictions{..}.normalize()`. **Invariant:** `Some(empty)`
is never constructible; smart constructors/decoders collapse empty → `None`;
well-formedness rejects `Some(empty)`.

WIT (`golem-core-v2.wit`):

```wit
variant numeric-bound { signed(s64), unsigned(u64), float-bits(u64) }
record numeric-restrictions {
    min: option<numeric-bound>, max: option<numeric-bound>, unit: option<string>,
}
// numeric cases gain a payload:
s8-type(option<numeric-restrictions>),  // … through f64-type(option<numeric-restrictions>)
```

Proto (`schema.proto`): numeric oneof fields 4–13 change
`golem.common.Empty` → `NumericRestrictions` (presence = `Some`; absent/empty
decodes/normalizes to `None`):

```proto
message NumericBound { oneof bound { int64 signed = 1; uint64 unsigned = 2; uint64 float_bits = 3; } }
message NumericRestrictions { NumericBound min = 1; NumericBound max = 2; optional string unit = 3; }
```

Validation semantics — centralized in one helper used by **every** entry point
(well-formedness, subtyping, equivalence, value validation, macro refinement), so
no path assumes a prior well-formedness pass (oracle finding 9):

```rust
fn normalize_numeric_restrictions_for_repr(
    repr: NumericRepr,                       // S8..U64, F32, F64
    r: Option<NumericRestrictions>,
) -> Result<Option<NumericRestrictions>, SchemaValidationError>;
```

- `well_formedness`: `min <= max` compared **numerically** (not by family tag or
  raw bits); bound family must match the repr (`U*` ⇒ `Unsigned`, `S*` ⇒
  `Signed`, `F*` ⇒ `FloatBits`) — family mismatch is an error; integer reprs
  reject fractional/float bounds; bounds must fit the repr range; reject
  `Some(empty)`; a `NumericBound` whose arm is set but value is non-finite is
  impossible (constructor guards), but a decoded malformed bound is rejected.
- `subtyping`: numeric narrowing within identical repr — sub.min ≥ sup.min,
  sub.max ≤ sup.max (`None` = unbounded, inclusive). Family mismatch ⇒ "not a
  subtype" (defended here too, not only in well-formedness). Equivalence compares
  normalized restrictions exactly. **`unit` is schema-level metadata** (numeric
  `SchemaValue`s carry no unit), so value validation never checks it; for
  equivalence/subtyping the normalized `unit` must match exactly (finding 10).
- `value`: **range only** against the node's restrictions; decode bound bits →
  `f64`/`i64`/`u64`, compare numerically. No unit check.

Float rule (finding 11): `FloatBits` stores **canonical f64 bits**. All
comparisons decode to `f64` and compare numerically, never by bit order. For
`F32`, bounds must round-trip through `f32` (well-formedness rejects bounds that
don't); value checks widen the `f32` value to `f64` and compare against the f64
bounds.

Wire/format notes (findings 6–8):
- **WIT** change `s8-type` → `s8-type(option<numeric-restrictions>)` is an
  intentional ABI change on an unreleased package. Unconstrained numeric encodes
  as `s8-type(none)`. There is no "missing payload decodes to None"; the binding
  type simply carries `Option`. `some(empty)` is accepted only at the decode
  boundary and normalized to `none`.
- **Proto** decode rules: a missing `SchemaType.body` is an invalid schema (not
  `S8(None)`); `Body::S8Type(NumericRestrictions::default())` decodes to
  `S8 { restrictions: None }`; `Some({min:None,max:None,unit:None|Some("")})`
  normalizes to `None`; a present `NumericBound` with no `bound` arm set is a
  decode error.
- **desert** `BinaryCodec` with `evolution()`: binary bytes for numeric schema
  variants are **not** backward-compatible across this unreleased refactor (a
  payloadless case gaining an `Option` payload is not assumed wire-stable). Phase
  1 adds a roundtrip test over `S8/U64/F64` plus adjacent `Char`/`String`/`Bool`
  to prove the codec is internally consistent after the change.

### F0.3 — `accepts-stdio` (D5), exact WIT

Add to `golem:tool` `common.wit` `positional` and `tail-positional` records:

```wit
accepts-stdio: bool,
```

Source of truth `sdks/rust/golem-rust/wit/deps/golem-tool/common.wit`; all 9
copies synced via `cargo make wit`.

### F0.4 — Subtree descriptor + graft (D6), exact shape

A trait associated fn with `where Self: Sized` is **not** callable from a bare
trait path (`<path::Remote>::…` needs a concrete `Self`), so the descriptor
cannot be a trait method that the parent reaches via the subtree trait path
(oracle blocker 1). Instead, `#[tool_definition] trait T` emits a **module-level
free function** with a deterministic name derived from the trait ident:

```rust
#[doc(hidden)]
pub fn __golem_tool_descriptor_for_T(
    ctx: &mut golem_rust::agentic::ToolBuildCtx,
) -> Result<golem_rust::agentic::ExtendedToolType, golem_rust::agentic::ToolBuildError>;
```

It also adds two hidden trait items so the `#[tool_implementation]` ctor and the
"must be implemented" check work exactly like agents:

```rust
#[doc(hidden)]
fn __tool_descriptor() -> golem_rust::agentic::ExtendedToolType where Self: Sized { // delegates to the free fn
    __golem_tool_descriptor_for_T(&mut golem_rust::agentic::ToolBuildCtx::new())
        .expect("tool descriptor build failed")
}
#[doc(hidden)]
fn tool_implementation_annotation() where Self: Sized;                              // forces #[tool_implementation]
```

`#[tool_implementation]` emits (via the re-exported `ctor`, like agents):

```rust
::golem_rust::ctor::__support::ctor_parse!(
    #[ctor] fn __register_tool_<lower>() {
        golem_rust::agentic::register_tool(<Impl as Trait>::__tool_descriptor());
    }
);
```

Because the ctor exists only on implemented types, an unimplemented tool is never
registered (D1). A subtree-only trait without its own `#[tool_implementation]` is
never a top-level tool (no ctor → not registered), but its free descriptor fn is
still reachable for grafting.

**Subtree resolution.** A method annotated `#[command(subtree = path::Remote)]`
(optionally `subtree = path::Remote, name = "remote"`) is rewritten by the parent
macro to call the child's free descriptor fn: it maps the path's last segment
`Remote` → `__golem_tool_descriptor_for_Remote` and calls
`path::__golem_tool_descriptor_for_Remote(ctx)`, threading the same `ctx`.

**`ToolBuildCtx`** (blocker 2) carries a recursion stack keyed by descriptor
identity (the free-fn path string) plus command path. Building a child:
- pushes the child identity; if already present, returns
  `ToolBuildError::SubtreeCycle(path)` (e.g. `git.remote -> remote.foo ->
  git.remote`) — no stack overflow;
- DAG reuse (same child grafted twice) is allowed: each graft **clones** the
  child `ExtendedToolType` and **remaps** its command indices and schema
  `type-node-index`es into the parent before merging — never shares mutable nodes;
- pops on return.

**Graft semantics** (blocker 3 — no silent loss):
- The child root command's `body` **must be `None`**; a child root with an
  executable body is rejected (`ToolBuildError::SubtreeRootHasBody`).
- The child root's `globals` are **copied onto the graft placeholder node** so
  they still apply to all descendants (per WIT recursive-globals semantics).
- The graft node's `name`/`doc`/`aliases`/`annotations` come from the parent
  subtree method's `#[command(...)]` when present, otherwise inherit from the
  child root. Name/alias uniqueness against siblings is validated after merge.
- The child root's subcommands (index-remapped) become the graft node's
  subcommands.
- Child-root-name rule: the child tool's root command name must equal the subtree
  method's command name (kebab) unless overridden by `name = "..."`; validated
  during the parent descriptor build.

### F0.5 — Canonical invocation-record field order (D7), restated authoritatively

The derived invocation record for a command body (used by validation, help, and
future dispatch/codegen) orders fields. Per the WIT, globals declared on a
command apply to that command's **own** body and to all descendants, so the
effective set runs from the root **through the current node inclusive** (oracle
blocker 5):

1. effective globals, root → current node **inclusive**, at each node
   **options then flags** (declaration order within each),
2. body fixed positionals (declaration order),
3. body tail positional (single list-typed field), if any,
4. body options (declaration order),
5. body flags (declaration order).

Worked example for `git remote add` (3-level):
1. `git` global options, `git` global flags,
2. `remote` global options, `remote` global flags,
3. `add` global options, `add` global flags,
4. `add` fixed positionals,
5. `add` tail positional,
6. `add` body options,
7. `add` body flags.

Globals are stored **once** in the declaring `command-node.globals` (D8) and are
**never** duplicated into any `command-body.options`/`flags`; the effective view
is derived by walking root→node. A validator rejects a body-local name colliding
with any effective (inherited or own) global.

### F0.6 — Defaults / `value-is` literals (D9)

`positional.default`, `option-spec.default`, and `value-is` literals are
`schema-value-tree`s. Macro flow: parse the Rust attr literal → resolve the
referenced arg's `SchemaType` → build a `SchemaValue` → encode with the schema
value encoder → store. Validated against the referenced type node at descriptor
build time. Enum default case names use the canonical kebab casing (F0.8).

### F0.7 — Error metadata via derive (D10), exact shape

```rust
#[derive(ToolError)]
enum GrepError {
    #[tool_error(kind = "usage-error", exit_code = 2)] BadPattern(String),
    #[tool_error(kind = "runtime-error", exit_code = 1)] Io(String),
}
```

Generates `impl golem_rust::agentic::ToolErrorSchema for GrepError` exposing per
variant: kebab `name`, `error-kind` (`usage-error`|`runtime-error`), `exit-code`
(`u8`), and the payload `SchemaType`. Tool methods returning `Result<T, E>`
require `E: ToolErrorSchema + Schema`; the macro reads error cases from
`E::error_cases()`. Payload rules per variant shape (oracle finding 13):
- unit variant → `payload: None`;
- exactly one field (named or unnamed) → payload `SchemaType` from that field via
  `Schema::get_type()`;
- two or more fields → **compile error** (no synthetic record in Phase 1–7).

### F0.8 — Canonical casing (D7/D9, §4.7 stability)

All tool-facing identifiers are kebab-case, matching the WIT regex
`^[a-z][a-z0-9]*(-[a-z0-9]+)*$`:

- tool name ← kebab(trait ident); root command name == tool name.
- command name ← kebab(method ident). Implicit body: the method whose snake_case
  == tool snake_case is the root body; `commands[0].name == tool.name` is enforced
  with a **compile error** on divergence (§5.8.1).
- option long / flag long / positional name ← kebab(param ident).
- error-case name ← kebab(variant ident); enum default case ← kebab(variant).

### F0.9 — Key-value / repeatable `Map` shaping (D4) + WIT `option-shape` split

The current WIT `option-shape::repeatable(repeatable-shape{repetition,%type})`
documents `%type` as the **element** of a list-collected option, and a
repeatable default as "a list whose elements are values of `%type`". A
`BTreeMap`-collected repeatable cannot be expressed under that invariant (its
collected value is a `Map`, not a `List`), so Phase 1 **changes the tool WIT**
(`golem:tool` common.wit, in scope of this PR) to model the collected shape
explicitly (oracle blocker 4):

```wit
variant option-shape {
  scalar(type-node-index),
  optional-scalar(type-node-index),
  repeatable-list(repeatable-list-shape),
  repeatable-map(repeatable-map-shape),
}
record repeatable-list-shape { repetition: repetition, item-type: type-node-index }
record repeatable-map-shape {
  repetition:          repetition,
  /// A `SchemaType::Map<K,V>` node; the collected value is this Map.
  map-type:            type-node-index,
  duplicate-key-policy: duplicate-key-policy,
}
enum duplicate-key-policy { reject, last-wins }
```

Shaping rules:
- `BTreeMap<String,V>` / `HashMap<String,V>` → `SchemaType::Map { key:String, value:V }`.
  Author git's `config` as `BTreeMap<String,String>` for deterministic order.
- repeatable **scalar** option (`Vec<T>`, grep `-e`): `repeatable-list(item-type =
  <T>)`; default is a `List<T>`; collected invocation value is `List<T>`.
- repeatable **key-value** option (git `config` as `BTreeMap`, `-c a=1 -c b=2`):
  `repeatable-map(map-type = <Map<K,V>>, duplicate-key-policy)`; default is a
  `Map<K,V>`; each `-c k=v` contributes one entry; collected invocation value is
  the merged `Map<K,V>`. Never `list<tuple>`.
- Defaults and `value-is` for each shape are values of the **collected** type
  (`List<T>` resp. `Map<K,V>`), consistent with D9 and the WIT invariant. For a
  `value-is` naming a `repeatable-list`, the literal is an element value (any
  occurrence equals it); for `repeatable-map`, the literal is an entry value.

### F0.10 — Runtime mirror targets (Phase 2), grounded

Mirror the agent pattern 1:1, with one deviation: the tool registry is
**deterministic** (Phase 6 snapshots `discover-tools`), so it stores tools in a
`BTreeMap<ToolName, ExtendedToolType>` (not the agent `HashMap`) and
`get_all_tools()` returns them sorted by root command name. Duplicate
registration of the same tool name **panics** with a clear message (catches two
impls for one tool) — oracle finding 12.
- `agent_registry.rs` (`static mut STATE`, `get_state()`, `register_agent_type`,
  `get_all_agent_types`) → `tool_registry.rs` (`register_tool`, `get_all_tools`,
  `get_tool_by_name`).
- `extended_agent_type.rs` (`ExtendedAgentType::to_agent_type()` merging per-method
  `SchemaGraph`s via `merge_agent_graphs` + `GraphEncoder`) → `extended_tool_type.rs`
  (`ExtendedToolType::to_tool()` merging per-arg graphs into one `tool.schema`,
  building the flattened command tree, **no synthetic input-record node**).
- `tool_impl.rs` `discover_tools`/`get_tool` wired to the registry (currently a
  placeholder returning empty / `InvalidToolName`).

## Risks

- Phase 1 cross-cuts `golem-schema` + protobuf + the `golem-core-v2` WIT + all
  four SDK codecs. The numeric wire-format change is a versioned evolution (not
  byte-identical to the current payloadless numeric cases); golden vectors and
  decoder normalization guard byte-equivalence.
- Subtree assembly is a runtime merge, not macro-time. The `#[command(subtree=…)]`
  authoring syntax and child-root-name rule must be settled in Phase 0 before the
  git example can be implemented.
- The `Option<NumericRestrictions>` form depends on the normalization invariant
  (D2) to keep derived `Eq` and §4.7 byte-equivalence correct; `Some(empty)` must
  never be constructible.
