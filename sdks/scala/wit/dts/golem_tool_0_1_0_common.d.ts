/**
 * Data model for Golem tool metadata.
 * A "tool" is a callable unit declared from a single piece of
 * metadata. From that metadata the SDKs project two delivered
 * surfaces — typed function signatures in TypeScript / Rust /
 * Python / Scala / MoonBit for programmatic invocation, and
 * help-text rendering for any node in the command tree — and the
 * same metadata is sufficient to drive a future full-CLI
 * projection (parseable args, shell completions, exit codes,
 * terminal-runnable as a Unix utility) without further authoring.
 * The CLI projection is a future possibility enabled by the model;
 * it is not part of the current specification's deliverable.
 * The model is CLI-native: commands, subcommands, options, flags, and
 * positionals are primary — not a generic data schema with CLI mappings
 * layered on top. Constraint-by-construction is preferred over runtime
 * validation: variadic-only-at-tail is structural; mutual exclusion is
 * expressed by subcommand choice or by an explicit `mutex-groups`
 * constraint; co-occurrence is structural via sub-records or via
 * `all-or-none`.
 * Types and values are not modeled by this package: every input/output
 * type and every metadata-time or runtime value is expressed with the
 * shared `golem:core/types@2.0.0` schema model. A tool owns a single
 * `schema-graph` type-node pool (the `tool.schema` field); command bodies
 * reference entries in it by `type-node-index`, exactly the same way the
 * agent model (`golem:agent/common`) references its per-agent
 * `schema-graph`. Metadata-time values (option/positional defaults, the
 * literal side of `value-is` constraint refs) are `schema-value-tree`s
 * interpreted against the referenced type node in `tool.schema`; runtime
 * invocation inputs, results, and custom-error payloads are self-contained
 * `typed-schema-value`s.
 * The remaining tool-specific recursion site is the command tree: a
 * flattened command hierarchy with the root at index 0 and children
 * referenced by `command-index`.
 * Construction invariants (validated by the producer; the WIT shape
 * alone does not enforce them):
 *   • All identifier-like strings (command names, option/flag long
 *     names, positional names, error names, formatter names) match
 *     `^[a-z][a-z0-9]*(-[a-z0-9]+)*$`.
 *   • Subcommand names + aliases are pairwise unique among siblings.
 *   • Within a `command-body`: option long-names + flag long-names +
 *     positional names + aliases + short forms are pairwise unique,
 *     AND unique against globals inherited from any ancestor command.
 *   • Constraint `ref` names resolve against body-declared options /
 *     flags / positionals AND globals inherited from any ancestor.
 *   • For `ref::value-is(name, lit)`, the literal must be a valid value
 *     for the declared type node of `name` in `tool.schema`.
 *   • `default-formatter` resolves to a name in `formatters`.
 *   • `tail-positional`: if `verbatim` is true, `separator` must be
 *     `some`; `separator` with `min = 0` is legal (the separator alone,
 *     no items, is valid).
 *   • A `positional` / `option` / `result` / `error` `type-node-index`
 *     resolves to a node in `tool.schema`.
 *   • A `repeatable` option's `default`, if present, is a list whose
 *     elements are values of the `repeatable-shape.%type` node.
 *   • A `value-is` ref naming a repeatable option, tail positional, or
 *     otherwise list-shaped target means "any occurrence / element
 *     equals this literal"; the literal is a value of the element type.
 *   • The tool's identity is its root command name
 *     (`commands.nodes[0].name`); `get-tool(name)` and
 *     `guest.invoke(tool-name, …)` match against it. `commands.nodes`
 *     is always non-empty.
 * Capability scoping (WASI preopens, env masking, outbound-socket
 * filters, subprocess-exec capability, and `golem:agent/host`'s
 * `get-config-value` resolution) is performed by the host by inspecting
 * the component's WIT imports — what a component *can* do is already
 * declared structurally by which interfaces it imports — not by reading
 * a declarative metadata record.
 */
declare module 'golem:tool/common@0.1.0' {
  import * as golemCore200Types from 'golem:core/types@2.0.0';
  import * as wasiIo023Streams from 'wasi:io/streams@0.2.3';
  export type SchemaGraph = golemCore200Types.SchemaGraph;
  export type TypeNodeIndex = golemCore200Types.TypeNodeIndex;
  export type SchemaValueTree = golemCore200Types.SchemaValueTree;
  export type TypedSchemaValue = golemCore200Types.TypedSchemaValue;
  export type OutputStream = wasiIo023Streams.OutputStream;
  /**
   * Command tree
   */
  export type CommandIndex = number;
  /**
   * Behavioral hints surfaced to MCP and other LLM-facing
   * surfaces. All four follow the MCP convention. When absent
   * the surface treats them as untrusted defaults
   * (`destructive: true`, `open-world: true`, `read-only: false`,
   * `idempotent: false`), per MCP guidance.
   */
  export type CommandAnnotations = {
    /** Tool performs no destructive updates; safe to call freely. */
    readOnly: boolean;
    /** Tool may delete or overwrite (default true per MCP semantics). */
    destructive: boolean;
    /** Repeated calls with the same input have the same effect. */
    idempotent: boolean;
    /**
     * Tool reaches outside the host's controlled environment
     * (network calls, external APIs, the open world).
     */
    openWorld: boolean;
  };
  export type Repetition = 
  /** --inc a --inc b */
  {
    tag: 'repeated'
  } |
  /** --inc=a,b */
  {
    tag: 'delimited'
    val: string
  } |
  /** Both surface forms accepted. */
  {
    tag: 'either'
    val: string
  };
  export type RepeatableShape = {
    repetition: Repetition;
    /** Index into `tool.schema`. */
    type: TypeNodeIndex;
  };
  export type OptionShape = 
  /** Required value: --opt VALUE or --opt=VALUE. Index into `tool.schema`. */
  {
    tag: 'scalar'
    val: TypeNodeIndex
  } |
  /**
   * Bare presence collapses to `default`; with value parses normally
   * (--decorate, --signed[=mode], --force-with-lease[=ref]). Index into
   * `tool.schema`.
   */
  {
    tag: 'optional-scalar'
    val: TypeNodeIndex
  } |
  /** Repeatable; value type in the derived signature is list-of-scalar. */
  {
    tag: 'repeatable'
    val: RepeatableShape
  };
  export type BoolFlagShape = {
    default_: boolean;
    /** If true, --no-<name> is auto-synthesized. */
    negatable: boolean;
  };
  export type FlagShape = 
  {
    tag: 'bool-flag'
    val: BoolFlagShape
  } |
  /** Counted flag (-vvv); optional max count. */
  {
    tag: 'count-flag'
    val: number | undefined
  };
  export type ValueIsRef = {
    name: string;
    /**
     * Literal value, interpreted against the declared type node of `name`
     * in `tool.schema`.
     */
    value: SchemaValueTree;
  };
  /**
   * Constraints
   */
  export type Ref = 
  {
    tag: 'present'
    val: string
  } |
  {
    tag: 'value-is'
    val: ValueIsRef
  };
  export type RefGroup = {
    refs: Ref[];
  };
  export type Quantifier = "all" | "any";
  export type ImpliesC = {
    lhsQuant: Quantifier;
    lhs: Ref[];
    rhsQuant: Quantifier;
    rhs: Ref[];
  };
  export type ForbidsC = {
    lhsQuant: Quantifier;
    lhs: Ref[];
    rhs: Ref[];
  };
  export type Constraint = 
  {
    tag: 'requires-all'
    val: Ref[]
  } |
  {
    tag: 'all-or-none'
    val: Ref[]
  } |
  {
    tag: 'requires-any'
    val: Ref[]
  } |
  {
    tag: 'mutex-groups'
    val: RefGroup[]
  } |
  {
    tag: 'implies'
    val: ImpliesC
  } |
  {
    tag: 'forbids'
    val: ForbidsC
  };
  export type ErrorKind = "usage-error" | "runtime-error";
  export type Example = {
    title: string;
    body: string;
  };
  /**
   * Documentation, examples
   */
  export type Doc = {
    summary: string;
    description: string;
    examples: Example[];
  };
  export type Positional = {
    name: string;
    doc: Doc;
    valueName?: string;
    /** Index into `tool.schema`. */
    type: TypeNodeIndex;
    /** Default value, interpreted against `%type` in `tool.schema`. */
    default_?: SchemaValueTree;
    required: boolean;
  };
  export type TailPositional = {
    name: string;
    doc: Doc;
    valueName?: string;
    /** Index into `tool.schema`. */
    itemType: TypeNodeIndex;
    min: number;
    max?: number;
    /** Token required before tail items (e.g. "--" for `git log -- <paths>`). */
    separator?: string;
    /**
     * If true, tokens after `separator` are not flag-parsed (for
     * `kubectl exec -- CMD ARGS...`).
     */
    verbatim: boolean;
  };
  /**
   * Positionals (variadic only at the tail, structurally)
   */
  export type Positionals = {
    fixed: Positional[];
    tail?: TailPositional;
  };
  /**
   * Options and flags
   */
  export type OptionSpec = {
    long: string;
    short?: string;
    aliases: string[];
    doc: Doc;
    valueName?: string;
    shape: OptionShape;
    /**
     * Default value, interpreted against the option's type node in
     * `tool.schema`.
     */
    default_?: SchemaValueTree;
    required: boolean;
    envVar?: string;
  };
  export type FlagSpec = {
    long: string;
    short?: string;
    aliases: string[];
    doc: Doc;
    shape: FlagShape;
    envVar?: string;
  };
  export type Globals = {
    options: OptionSpec[];
    flags: FlagSpec[];
  };
  /**
   * Streams, structured results, errors
   */
  export type StreamSpec = {
    doc: Doc;
    mime: string[];
    required: boolean;
  };
  export type Formatter = {
    name: string;
    doc: Doc;
  };
  export type ResultSpec = {
    /** Index into `tool.schema`. */
    type: TypeNodeIndex;
    doc: Doc;
    formatters: Formatter[];
    defaultFormatter: string;
  };
  export type ErrorCase = {
    name: string;
    doc: Doc;
    kind: ErrorKind;
    exitCode: number;
    /** Index into `tool.schema`. */
    payload?: TypeNodeIndex;
  };
  export type CommandBody = {
    positionals: Positionals;
    options: OptionSpec[];
    flags: FlagSpec[];
    constraints: Constraint[];
    stdin?: StreamSpec;
    stdout?: StreamSpec;
    result?: ResultSpec;
    errors: ErrorCase[];
    annotations?: CommandAnnotations;
  };
  /**
   * A command may dispatch to subcommands, run its own body, or both.
   * Globals declared here apply to this command's own body and to
   * every descendant subcommand body (recursive globals — "this level
   * and downward").
   */
  export type CommandNode = {
    name: string;
    aliases: string[];
    doc: Doc;
    globals: Globals;
    subcommands: CommandIndex[];
    body?: CommandBody;
  };
  export type CommandTree = {
    /** Always non-empty; the root command is at index 0. */
    nodes: CommandNode[];
  };
  /**
   * Top level
   */
  export type Tool = {
    version: string;
    commands: CommandTree;
    /**
     * Self-contained type-node pool holding every type referenced from
     * this tool's commands. Command bodies reference entries by
     * `type-node-index`. `schema.root` is a structurally-required
     * placeholder and is not the semantic root of the tool (mirrors
     * `golem:agent/common`'s `agent-type.schema`).
     */
    schema: SchemaGraph;
  };
  /**
   * Invocation contract — shared between guest and host
   */
  export type ToolError = 
  {
    tag: 'invalid-tool-name'
    val: string
  } |
  {
    tag: 'invalid-command-path'
    val: string[]
  } |
  {
    tag: 'invalid-input'
    val: string
  } |
  {
    tag: 'constraint-violation'
    val: string
  } |
  /**
   * Returned `invocation-result` does not match the body's
   * declared `result-spec` (e.g., the returned value's root type
   * does not match the body's declared result schema; see §6.1
   * transparency invariant).
   */
  {
    tag: 'invalid-result'
    val: string
  } |
  /**
   * Tool-defined failure. Mirrors `golem:agent/common`'s
   * `agent-error::custom-error`: the payload is a self-contained
   * `typed-schema-value` carrying the error value. Producers SHOULD
   * shape it so its root type matches one of the body's declared
   * `error-case` payload types.
   */
  {
    tag: 'custom-error'
    val: TypedSchemaValue
  };
  export type InvocationResult = {
    result?: TypedSchemaValue;
    stdout?: OutputStream;
  };
}
