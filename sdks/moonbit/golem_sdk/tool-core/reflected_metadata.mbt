///|
/// Reconstructs the dynamic authoring model only when reflection requests it.
pub fn ToolDef::from_wire(
  tool : @toolCommon.Tool,
) -> ToolDef raise ToolBuildError {
  try {
    let graph = (root : Int) => {
      @model.schema_graph_from_wit({ ..tool.schema, root, })
    }
    let value = (tree : @types.SchemaValueTree) => {
      @model.schema_value_from_wit(tree, @model_host.resource_drops())
    }
    let refs = (items : Array[@toolCommon.Ref]) => {
      items.map(item => {
        match item {
          @toolCommon.Present(name) => RefDef::Present(name)
          @toolCommon.ValueIs(spec) =>
            RefDef::ValueIs({
              name: spec.name,
              value: Resolved(value(spec.value)),
            })
        }
      })
    }
    let option = (spec : @toolCommon.OptionSpec) => {
      let shape = match spec.shape {
        @toolCommon.Scalar(index) => OptionShapeDef::Scalar(graph(index))
        @toolCommon.OptionalScalar(index) =>
          OptionShapeDef::OptionalScalar(graph(index))
        @toolCommon.RepeatableList(spec) =>
          OptionShapeDef::RepeatableList({
            repetition: spec.repetition,
            item_type: graph(spec.item_type),
          })
        @toolCommon.RepeatableMap(spec) =>
          OptionShapeDef::RepeatableMap({
            repetition: spec.repetition,
            map_type: graph(spec.map_type),
            duplicate_key_policy: spec.duplicate_key_policy,
          })
      }
      OptionSpecDef::{
        long: spec.long,
        short: spec.short,
        aliases: spec.aliases,
        doc: spec.doc,
        value_name: spec.value_name,
        shape,
        default: spec.default.map(value),
        required: spec.required,
        env_var: spec.env_var,
      }
    }
    {
      version: tool.version,
      commands: tool.commands.nodes.map(node => CommandNodeDef::{
        name: node.name,
        aliases: node.aliases,
        doc: node.doc,
        globals: {
          options: node.globals.options.map(option),
          flags: node.globals.flags,
        },
        subcommands: node.subcommands,
        body: node.body.map(body => CommandBodyDef::{
          positionals: {
            fixed: body.positionals.fixed.map(p => PositionalDef::{
              name: p.name,
              doc: p.doc,
              value_name: p.value_name,
              type_: graph(p.type_),
              default: p.default.map(value),
              required: p.required,
              accepts_stdio: p.accepts_stdio,
            }),
            tail: body.positionals.tail.map(p => TailPositionalDef::{
              name: p.name,
              doc: p.doc,
              value_name: p.value_name,
              item_type: graph(p.item_type),
              min: p.min,
              max: p.max,
              separator: p.separator,
              verbatim: p.verbatim,
              accepts_stdio: p.accepts_stdio,
            }),
          },
          options: body.options.map(option),
          flags: body.flags,
          constraints: body.constraints.map(c => {
            match c {
              @toolCommon.RequiresAll(r) => ConstraintDef::RequiresAll(refs(r))
              @toolCommon.RequiresAny(r) => ConstraintDef::RequiresAny(refs(r))
              @toolCommon.AllOrNone(r) => ConstraintDef::AllOrNone(refs(r))
              @toolCommon.MutexGroups(groups) =>
                ConstraintDef::MutexGroups(
                  groups.map(g => { refs: refs(g.refs), }),
                )
              @toolCommon.Implies(spec) =>
                ConstraintDef::Implies({
                  lhs_quant: spec.lhs_quant,
                  lhs: refs(spec.lhs),
                  rhs_quant: spec.rhs_quant,
                  rhs: refs(spec.rhs),
                })
              @toolCommon.Forbids(spec) =>
                ConstraintDef::Forbids({
                  lhs_quant: spec.lhs_quant,
                  lhs: refs(spec.lhs),
                  rhs: refs(spec.rhs),
                })
            }
          }),
          stdin: body.stdin,
          stdout: body.stdout,
          result: body.result.map(r => ResultSpecDef::{
            type_: graph(r.type_),
            doc: r.doc,
            formatters: r.formatters,
            default_formatter: r.default_formatter,
          }),
          errors: body.errors.map(e => ErrorCaseDef::{
            name: e.name,
            doc: e.doc,
            kind: e.kind,
            exit_code: e.exit_code,
            payload: e.payload.map(graph),
          }),
          annotations: body.annotations,
        }),
      }),
    }
  } catch {
    error => raise SchemaModelError(repr(error))
  }
}
