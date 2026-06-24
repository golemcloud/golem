Data model for Golem tool metadata.

A "tool" is a callable unit declared from a single piece of
metadata. From that metadata the SDKs project two delivered
surfaces — typed function signatures in TypeScript / Rust /
Python / Scala / MoonBit for programmatic invocation, and
help-text rendering for any node in the command tree — and the
same metadata is sufficient to drive a future full-CLI
projection (parseable args, shell completions, exit codes,
terminal-runnable as a Unix utility) without further authoring.
The CLI projection is a future possibility enabled by the model;
it is not part of the current specification's deliverable.

The model is CLI-native: commands, subcommands, options, flags, and
positionals are primary — not a generic data schema with CLI mappings
layered on top. Constraint-by-construction is preferred over runtime
validation: variadic-only-at-tail is structural; mutual exclusion is
expressed by subcommand choice or by an explicit `mutex-groups`
constraint; co-occurrence is structural via sub-records or via
`all-or-none`.

Types and values are not modeled by this package: every input/output
type and every metadata-time or runtime value is expressed with the
shared `golem:core/types@2.0.0` schema model. A tool owns a single
`schema-graph` type-node pool (the `tool.schema` field); command bodies
reference entries in it by `type-node-index`, exactly the same way the
agent model (`golem:agent/common`) references its per-agent
`schema-graph`. Metadata-time values (option/positional defaults, the
literal side of `value-is` constraint refs) are `schema-value-tree`s
interpreted against the referenced type node in `tool.schema`; runtime
invocation inputs, results, and custom-error payloads are self-contained
`typed-schema-value`s.

The remaining tool-specific recursion site is the command tree: a
flattened command hierarchy with the root at index 0 and children
referenced by `command-index`.

Construction invariants (validated by the producer; the WIT shape
alone does not enforce them):

  • All identifier-like strings (command names, option/flag long
    names, positional names, error names, formatter names) match
    `^[a-z][a-z0-9]*(-[a-z0-9]+)*$`.
  • Subcommand names + aliases are pairwise unique among siblings.
  • Within a `command-body`: option long-names + flag long-names +
    positional names + aliases + short forms are pairwise unique,
    AND unique against globals inherited from any ancestor command.
  • Constraint `ref` names resolve against body-declared options /
    flags / positionals AND globals inherited from any ancestor.
  • For `ref::value-is(name, lit)`, the literal must be a valid value
    for the declared type node of `name` in `tool.schema`.
  • `default-formatter` resolves to a name in `formatters`.
  • `tail-positional`: if `verbatim` is true, `separator` must be
    `some`; `separator` with `min = 0` is legal (the separator alone,
    no items, is valid).
  • A `positional` / `option` / `result` / `error` `type-node-index`
    resolves to a node in `tool.schema`.
  • A `repeatable` option's `default`, if present, is a list whose
    elements are values of the `repeatable-shape.%type` node.
  • A `value-is` ref naming a repeatable option, tail positional, or
    otherwise list-shaped target means "any occurrence / element
    equals this literal"; the literal is a value of the element type.
  • The tool's identity is its root command name
    (`commands.nodes[0].name`); `get-tool(name)` and
    `guest.invoke(tool-name, …)` match against it. `commands.nodes`
    is always non-empty.

Capability scoping (WASI preopens, env masking, outbound-socket
filters, subprocess-exec capability, and `golem:agent/host`'s
`get-config-value` resolution) is performed by the host by inspecting
the component's WIT imports — what a component *can* do is already
declared structurally by which interfaces it imports — not by reading
a declarative metadata record.