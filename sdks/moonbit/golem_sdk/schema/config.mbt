///|
fn canonical_config_name(name : String) -> String {
  let buffer = StringBuilder()
  let mut upper = false
  for ch in name {
    if ch == '_' {
      upper = true
    } else {
      buffer.write_char(
        if upper && ch >= 'a' && ch <= 'z' {
          (ch.to_int() - 32).unsafe_to_char()
        } else {
          ch
        },
      )
      upper = false
    }
  }
  buffer.to_string()
}

///|
// Project only config graphs. Positional codecs and ordinary method schemas
// retain their native fields; discriminator literals and catalog paths are data.
pub fn canonical_config_graph(graph : @types.SchemaGraph) -> @types.SchemaGraph {
  let nodes = graph.type_nodes.map(node => {
    let body = match node.body {
      @types.RecordType(fields) => {
        let names : Map[String, Unit] = Map([])
        @types.RecordType(
          fields.map(field => {
            let name = canonical_config_name(field.name)
            if name.is_empty() || name.has_prefix("$") || names.contains(name) {
              abort("invalid or duplicate canonical config field: " + name)
            }
            names[name] = ()
            { ..field, name, }
          }),
        )
      }
      @types.UnionType(spec) =>
        @types.UnionType({
          branches: spec.branches.map(branch => {
            let discriminator = match branch.discriminator {
              @types.FieldEquals(field) =>
                @types.FieldEquals({
                  ..field,
                  field_name: canonical_config_name(field.field_name),
                })
              @types.FieldAbsent(name) =>
                @types.FieldAbsent(canonical_config_name(name))
              other => other
            }
            { ..branch, discriminator, }
          }),
        })
      other => other
    }
    { ..node, body, }
  })
  // A recursive type reused by a method must not share the projected definition.
  let defs = if nodes == graph.type_nodes {
    graph.defs
  } else {
    graph.defs.map(def => { ..def, id: def.id + "/config", })
  }
  { ..graph, type_nodes: nodes, defs, }
}
