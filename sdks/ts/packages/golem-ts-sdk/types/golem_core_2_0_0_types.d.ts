declare module 'golem:core/types@2.0.0' {
  /**
   * Parses a UUID from a string
   * @throws string
   */
  export function parseUuid(uuid: string): Uuid;
  /**
   * Converts a UUID to a string
   */
  export function uuidToString(uuid: Uuid): string;
  /**
   * ============================================================
   * Carrier indices
   * ============================================================
   * Index into a `schema-graph`'s `type-nodes` list.
   */
  export type TypeNodeIndex = number;
  /**
   * Index into a `schema-value-tree`'s `value-nodes` list.
   */
  export type ValueNodeIndex = number;
  /**
   * Index into a `schema-graph`'s `defs` list.
   */
  export type DefIndex = number;
  /**
   * Stable, language-independent identifier for a named type definition.
   * Must be unique within the enclosing `schema-graph`. Conventional format
   * is a dot-separated namespace path (e.g., `"myapp.users.user"`). Each
   * SDK provides a default derivation rule (typically based on the local
   * language's type name); cross-language interop requires the same
   * `type-id` on every side, which users can pin via the SDK's `named`
   * attribute.
   */
  export type TypeId = string;
  /**
   * ============================================================
   * Common embedded value types
   * ============================================================
   */
  export type Uuid = {
    highBits: bigint;
    lowBits: bigint;
  };
  export type Datetime = {
    /** Seconds since the Unix epoch (UTC). */
    seconds: bigint;
    /** Nanoseconds since `seconds`, in `[0, 1_000_000_000)`. */
    nanoseconds: number;
  };
  export type EnvironmentId = {
    uuid: Uuid;
  };
  /**
   * ============================================================
   * Platform identifiers
   * ============================================================
   * Platform identifiers used by the schema-native surfaces. Keeping them
   * here lets migrated interfaces depend on a single core version.
   * Represents a Golem component
   */
  export type ComponentId = {
    uuid: Uuid;
  };
  /**
   * Represents a Golem agent
   */
  export type AgentId = {
    /** Identifies the component the agent belongs to */
    componentId: ComponentId;
    /** String representation of the agent ID (agent type and constructor parameters) */
    agentId: string;
  };
  /**
   * Represents a Golem account
   */
  export type AccountId = {
    uuid: Uuid;
  };
  /**
   * An index into the persistent log storing all performed operations of an agent
   */
  export type OplogIndex = bigint;
  /**
   * A promise ID is a value that can be passed to an external Golem API to
   * complete that promise from an arbitrary external source, while Golem
   * agents can await for this completion.
   */
  export type PromiseId = {
    agentId: AgentId;
    oplogIdx: OplogIndex;
  };
  /**
   * A named type definition inside a `schema-graph`.
   * The def itself does not carry metadata; metadata lives on the
   * referenced `schema-type-node` so there is one source of truth for
   * docs / aliases / examples / deprecation / role on each type.
   */
  export type SchemaTypeDef = {
    /** Stable identifier; unique within the enclosing graph. */
    id: TypeId;
    /** Optional human-readable qualified name (display only). */
    name?: string;
    /** Index into the enclosing graph's `type-nodes` for this def's body. */
    body: TypeNodeIndex;
  };
  /**
   * Open registry; unknown roles fall back to structural handling.
   */
  export type Role = 
  /** `list<variant<…>>` whose elements are interchangeable modalities. */
  {
    tag: 'multimodal'
  } |
  /** `variant { inline: text, url: url }` ergonomic unstructured-text wrapper. */
  {
    tag: 'unstructured-text'
  } |
  /** `variant { inline: binary, url: url }` ergonomic unstructured-binary wrapper. */
  {
    tag: 'unstructured-binary'
  } |
  /** Any other producer-defined role, preserved verbatim. */
  {
    tag: 'other'
    val: string
  };
  /**
   * Typed metadata envelope. Holds non-validation, non-rendering-critical
   * information (docs, aliases, examples, deprecation, role). Per-scalar
   * validation constraints live on the relevant scalar's typed substructure,
   * not here.
   */
  export type MetadataEnvelope = {
    doc?: string;
    aliases: string[];
    /**
     * Canonical-encoded example values (JSON strings). Empty = no examples.
     * Kept as strings so metadata is self-contained on the type side and
     * does not have to cross-reference an accompanying value tree.
     */
    examples: string[];
    /** Deprecation message; `none` means not deprecated. */
    deprecated?: string;
    /** Optional role annotation tagging a type with a consumer-facing intent. */
    role?: Role;
  };
  export type NamedFieldType = {
    name: string;
    body: TypeNodeIndex;
    metadata: MetadataEnvelope;
  };
  export type VariantCaseType = {
    name: string;
    payload?: TypeNodeIndex;
    metadata: MetadataEnvelope;
  };
  export type FixedListSpec = {
    element: TypeNodeIndex;
    length: number;
  };
  /**
   * Map key types are restricted to primitives. Enforced at schema
   * construction time, not by the WIT type itself.
   */
  export type MapSpec = {
    key: TypeNodeIndex;
    value: TypeNodeIndex;
  };
  export type ResultSpec = {
    ok?: TypeNodeIndex;
    err?: TypeNodeIndex;
  };
  /**
   * --- Text / Binary restrictions ---
   */
  export type TextRestrictions = {
    /** Optional set of allowed BCP-47 language codes. `none` = unrestricted. */
    languages?: string[];
    minLength?: number;
    maxLength?: number;
    regex?: string;
  };
  export type BinaryRestrictions = {
    /** Optional set of allowed MIME types. `none` = unrestricted. */
    mimeTypes?: string[];
    minBytes?: number;
    maxBytes?: number;
  };
  /**
   * --- Path ---
   */
  export type PathDirection = "input" | "output" | "in-out";
  export type PathKind = "file" | "directory" | "any";
  export type PathSpec = {
    direction: PathDirection;
    kind: PathKind;
    allowedMimeTypes?: string[];
    allowedExtensions?: string[];
  };
  /**
   * --- URL ---
   */
  export type UrlRestrictions = {
    allowedSchemes?: string[];
    allowedHosts?: string[];
  };
  /**
   * --- Quantity ---
   * Fixed-point decimal value with unit: numeric value = `mantissa * 10^(-scale)`.
   * `unit` is a free-form string at the value level; the schema's
   * `quantity-spec` constrains the accepted set.
   */
  export type QuantityValue = {
    mantissa: bigint;
    scale: number;
    unit: string;
  };
  export type QuantitySpec = {
    /** Canonical base unit (e.g., `"kg"`, `"m"`, `"s"`, `"B"`). */
    baseUnit: string;
    /** Suffixes accepted on input and rendered on output (e.g., `["kg","g","mg"]`). */
    allowedSuffixes: string[];
    /** Optional inclusive range, expressed in canonical fixed-point form. */
    min?: QuantityValue;
    max?: QuantityValue;
  };
  export type FieldDiscriminator = {
    fieldName: string;
    /** Optional required literal value for the field. */
    literal?: string;
  };
  /**
   * How the decoder identifies that a value belongs to a given union branch.
   */
  export type DiscriminatorRule = 
  /** String value starts with this prefix (e.g. `"ssh://"`). */
  {
    tag: 'prefix'
    val: string
  } |
  /** String value ends with this suffix (e.g. `".tar.gz"`). */
  {
    tag: 'suffix'
    val: string
  } |
  /** String value contains this substring. */
  {
    tag: 'contains'
    val: string
  } |
  /** String value matches this anchored regex. */
  {
    tag: 'regex'
    val: string
  } |
  /**
   * Record-shaped value where the named field is present, and — if
   * `literal` is set — has the given literal string value. The common
   * JSON-discriminated-object case (`{"kind":"circle",…}`) is
   * `field-equals { field-name: "kind", literal: some("circle") }`.
   */
  {
    tag: 'field-equals'
    val: FieldDiscriminator
  } |
  /** Record-shaped value where the named field is absent. */
  {
    tag: 'field-absent'
    val: string
  };
  export type UnionBranch = {
    /**
     * Logical branch name. Carried in `union-value.tag` after the
     * decoder resolves the branch; used by renderers, codegen, docs.
     */
    tag: string;
    /**
     * Branch body type. Any schema type compatible with the
     * discriminator rule (see schema-construction validation above).
     */
    body: TypeNodeIndex;
    /** Rule the decoder uses to pick this branch from a raw value. */
    discriminator: DiscriminatorRule;
    metadata: MetadataEnvelope;
  };
  /**
   * --- Discriminated union ---
   * Inferred-tag sum. Each branch declares a rule that the decoder uses to
   * identify values belonging to that branch from the raw underlying value.
   * Rules enforced at schema-construction time:
   *   - branch `tag`s are unique within the union,
   *   - string-pattern discriminators (`prefix` / `suffix` / `contains` /
   *     `regex`) require a string-shaped branch body (`string-type`,
   *     `text-type`, `url-type`, `path-type`, or a `ref` resolving to one),
   *   - record-shaped discriminators (`field-equals` / `field-absent`)
   *     require a record-shaped branch body that declares the referenced
   *     field (with the matching literal type, if any),
   *   - the discriminator set must be unambiguous: no value matches more
   *     than one branch. Overlap is checked structurally where decidable
   *     (e.g., `prefix("a")` and `prefix("ab")` overlap) and best-effort
   *     for `regex`.
   */
  export type UnionSpec = {
    branches: UnionBranch[];
  };
  /**
   * --- Capability nodes ---
   */
  export type SecretSpec = {
    /** Optional categorisation (e.g., `"api-key"`, `"oauth-token"`). */
    category?: string;
  };
  export type QuotaTokenSpec = {
    /**
     * Resource name this token covers (e.g., declared in the agent manifest).
     * `none` = any resource permitted.
     */
    resourceName?: string;
  };
  /**
   * The structural body of a `schema-type-node`.
   * Closed sum types come in two shapes that differ by how the decoder
   * learns which branch a value belongs to:
   *   - `variant-type` is a **carried-tag** sum: the value explicitly
   *     carries its case (`variant-value.case` index). Zero-inference
   *     decoding. Natural mapping for language-level algebraic data types
   *     (Rust `enum`, Scala `sealed trait`, WIT `variant`, etc.).
   *   - `union-type` is an **inferred-tag** sum: the value does not carry
   *     its tag. Each branch declares a `discriminator-rule` (prefix /
   *     suffix / contains / regex on string-shaped bodies, or
   *     field-equals / field-absent on record-shaped bodies) and the
   *     decoder picks the branch whose rule matches the raw value.
   *     Natural mapping for inputs where the producer writes an unadorned
   *     value (a URL whose scheme picks the handler, a JSON object whose
   *     `"kind"` field picks the variant, an MCP content block whose
   *     `"type"` field picks the part shape, …).
   */
  export type SchemaTypeBody = 
  /** --- Reference to a named definition in the same `schema-graph` --- */
  {
    tag: 'ref-type'
    val: DefIndex
  } |
  /** --- Primitives --- */
  {
    tag: 'bool-type'
  } |
  {
    tag: 's8-type'
  } |
  {
    tag: 's16-type'
  } |
  {
    tag: 's32-type'
  } |
  {
    tag: 's64-type'
  } |
  {
    tag: 'u8-type'
  } |
  {
    tag: 'u16-type'
  } |
  {
    tag: 'u32-type'
  } |
  {
    tag: 'u64-type'
  } |
  {
    tag: 'f32-type'
  } |
  {
    tag: 'f64-type'
  } |
  {
    tag: 'char-type'
  } |
  {
    tag: 'string-type'
  } |
  /** --- Structural composites --- */
  {
    tag: 'record-type'
    val: NamedFieldType[]
  } |
  {
    tag: 'variant-type'
    val: VariantCaseType[]
  } |
  {
    tag: 'enum-type'
    val: string[]
  } |
  {
    tag: 'flags-type'
    val: string[]
  } |
  {
    tag: 'tuple-type'
    val: TypeNodeIndex[]
  } |
  {
    tag: 'list-type'
    val: TypeNodeIndex
  } |
  {
    tag: 'fixed-list-type'
    val: FixedListSpec
  } |
  {
    tag: 'map-type'
    val: MapSpec
  } |
  {
    tag: 'option-type'
    val: TypeNodeIndex
  } |
  {
    tag: 'result-type'
    val: ResultSpec
  } |
  /** --- Rich semantic types --- */
  {
    tag: 'text-type'
    val: TextRestrictions
  } |
  {
    tag: 'binary-type'
    val: BinaryRestrictions
  } |
  {
    tag: 'path-type'
    val: PathSpec
  } |
  {
    tag: 'url-type'
    val: UrlRestrictions
  } |
  {
    tag: 'datetime-type'
  } |
  {
    tag: 'duration-type'
  } |
  {
    tag: 'quantity-type'
    val: QuantitySpec
  } |
  /** --- Discriminated union (closed, inferred-tag) --- */
  {
    tag: 'union-type'
    val: UnionSpec
  } |
  /** --- Capability nodes --- */
  {
    tag: 'secret-type'
    val: SecretSpec
  } |
  {
    tag: 'quota-token-type'
    val: QuotaTokenSpec
  } |
  /** --- WASI P3 stubs (parseable only; no semantics yet) --- */
  {
    tag: 'future-type'
    val: TypeNodeIndex | undefined
  } |
  {
    tag: 'stream-type'
    val: TypeNodeIndex | undefined
  };
  /**
   * ============================================================
   * Schema type
   * ============================================================
   * One node in a `schema-graph`. Carries the type body and a per-node
   * metadata envelope (docs, aliases, examples, deprecation, role).
   * Recursive positions reference other nodes (or named definitions) by
   * index inside the body.
   */
  export type SchemaTypeNode = {
    body: SchemaTypeBody;
    metadata: MetadataEnvelope;
  };
  /**
   * ============================================================
   * Schema graph (self-contained type carrier)
   * ============================================================
   * A self-contained schema graph. Anywhere a schema travels with a value
   * (typed pair, oplog `custom` payload, REST/RPC envelope, public oplog
   * rendering), the payload owns its own `schema-graph` — there is no
   * implicit external registry that consumers must look up.
   */
  export type SchemaGraph = {
    /**
     * All schema-type nodes used in this graph. Reachable from `root`
     * directly (anonymous types) or transitively via `defs`.
     */
    typeNodes: SchemaTypeNode[];
    /**
     * Named type definitions in this graph. Indices into this list are
     * the targets of `schema-type-node::ref-type`. Ordering is
     * deterministic (sorted by `id`).
     */
    defs: SchemaTypeDef[];
    /** Index into `type-nodes` of the root schema type. */
    root: TypeNodeIndex;
  };
  export type VariantValuePayload = {
    case_: number;
    payload?: ValueNodeIndex;
  };
  export type MapEntry = {
    key: ValueNodeIndex;
    value: ValueNodeIndex;
  };
  /**
   * Result payload: exactly one of `ok-value` / `err-value` is set. Each
   * inner option allows `result<_, _>` cases whose ok/err type is unit (no
   * payload).
   */
  export type ResultValuePayload = 
  {
    tag: 'ok-value'
    val: ValueNodeIndex | undefined
  } |
  {
    tag: 'err-value'
    val: ValueNodeIndex | undefined
  };
  export type TextValuePayload = {
    text: string;
    /** BCP-47 language tag, when known. */
    language?: string;
  };
  export type BinaryValuePayload = {
    bytes: Uint8Array;
    mimeType?: string;
  };
  /**
   * Signed duration as total nanoseconds.
   */
  export type DurationValuePayload = {
    nanoseconds: bigint;
  };
  export type UnionValuePayload = {
    /**
     * Tag of the branch the decoder resolved, matching one of the
     * `union-spec::branches[*].tag` values. Carried so receivers do not
     * have to re-run discriminator rules to know which branch was
     * matched; encoders must ensure it agrees with the body.
     */
    tag: string;
    /**
     * Underlying value. Its shape matches the resolved branch's body
     * type and (by construction) satisfies the branch's discriminator
     * rule.
     */
    body: ValueNodeIndex;
  };
  /**
   * Capability value: secret transport is **by reference**. The schema side
   * declares the secret; the value side carries an opaque reference that the
   * authority resolves on read. The literal secret material never crosses
   * this carrier.
   */
  export type SecretValuePayload = {
    /** Opaque, authority-resolved reference. */
    secretRef: string;
  };
  /**
   * Capability value: quota-token transport is **by snapshot**. The receiver
   * re-acquires a live lease against `(environment-id, resource-name)` on
   * demand.
   */
  export type QuotaTokenValuePayload = {
    environmentId: EnvironmentId;
    resourceName: string;
    expectedUse: bigint;
    lastCredit: bigint;
    lastCreditAt: Datetime;
  };
  export type SchemaValueNode = 
  /** Primitives */
  {
    tag: 'bool-value'
    val: boolean
  } |
  {
    tag: 's8-value'
    val: number
  } |
  {
    tag: 's16-value'
    val: number
  } |
  {
    tag: 's32-value'
    val: number
  } |
  {
    tag: 's64-value'
    val: bigint
  } |
  {
    tag: 'u8-value'
    val: number
  } |
  {
    tag: 'u16-value'
    val: number
  } |
  {
    tag: 'u32-value'
    val: number
  } |
  {
    tag: 'u64-value'
    val: bigint
  } |
  {
    tag: 'f32-value'
    val: number
  } |
  {
    tag: 'f64-value'
    val: number
  } |
  {
    tag: 'char-value'
    val: string
  } |
  {
    tag: 'string-value'
    val: string
  } |
  /** Structural composites */
  {
    tag: 'record-value'
    val: ValueNodeIndex[]
  } |
  {
    tag: 'variant-value'
    val: VariantValuePayload
  } |
  {
    tag: 'enum-value'
    val: number
  } |
  {
    tag: 'flags-value'
    val: boolean[]
  } |
  {
    tag: 'tuple-value'
    val: ValueNodeIndex[]
  } |
  {
    tag: 'list-value'
    val: ValueNodeIndex[]
  } |
  {
    tag: 'fixed-list-value'
    val: ValueNodeIndex[]
  } |
  {
    tag: 'map-value'
    val: MapEntry[]
  } |
  {
    tag: 'option-value'
    val: ValueNodeIndex | undefined
  } |
  {
    tag: 'result-value'
    val: ResultValuePayload
  } |
  /** Rich semantic */
  {
    tag: 'text-value'
    val: TextValuePayload
  } |
  {
    tag: 'binary-value'
    val: BinaryValuePayload
  } |
  {
    tag: 'path-value'
    val: string
  } |
  {
    tag: 'url-value'
    val: string
  } |
  {
    tag: 'datetime-value'
    val: Datetime
  } |
  {
    tag: 'duration-value'
    val: DurationValuePayload
  } |
  {
    tag: 'quantity-value-node'
    val: QuantityValue
  } |
  /** Discriminated union: tag is matched against schema branches. */
  {
    tag: 'union-value'
    val: UnionValuePayload
  } |
  /** Capability nodes */
  {
    tag: 'secret-value'
    val: SecretValuePayload
  } |
  {
    tag: 'quota-token-value'
    val: QuotaTokenValuePayload
  };
  /**
   * ============================================================
   * Schema value (always paired with a schema-graph)
   * ============================================================
   * A flat schema-value tree. Always travels paired with a `schema-graph`
   * (see `typed-schema-value`). Indices refer to entries in `value-nodes`
   * within this same tree.
   * The value tree is structurally driven by the schema: record-value
   * payload order matches the schema's field order, variant-value carries a
   * case index, enum-value carries a case index, union-value carries the
   * discriminator's literal tag. The value side does not redundantly carry
   * field names, case names, or named-ref identifiers — those come from the
   * schema.
   */
  export type SchemaValueTree = {
    valueNodes: SchemaValueNode[];
    root: ValueNodeIndex;
  };
  /**
   * ============================================================
   * Wire carriers
   * ============================================================
   * A typed value: a self-contained schema graph paired with a value tree
   * built against that schema.
   */
  export type TypedSchemaValue = {
    graph: SchemaGraph;
    value: SchemaValueTree;
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
