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

///|
fn append_config_type(
  encoder : @schema.WireTypeEncoder,
  graph : @types.SchemaGraph,
  index : Int32,
) -> Int32 {
  let node = graph.type_nodes[index]
  match node.body {
    @types.RefType(index) => {
      let def = graph.defs[index]
      encoder.named(
        def.id,
        () => append_config_type(encoder, graph, def.body),
        name=def.name,
      )
    }
    body =>
      encoder.push_with_metadata(
        map_config_children(body, child => {
          append_config_type(encoder, graph, child)
        }),
        node.metadata,
      )
  }
}

///|
fn map_config_children(
  body : @types.SchemaTypeBody,
  f : (Int32) -> Int32,
) -> @types.SchemaTypeBody {
  match body {
    @types.RecordType(fields) =>
      @types.RecordType(fields.map(field => { ..field, body: f(field.body), }))
    @types.VariantType(cases) =>
      @types.VariantType(
        cases.map(case => { ..case, payload: case.payload.map(f), }),
      )
    @types.TupleType(items) => @types.TupleType(items.map(f))
    @types.ListType(item) => @types.ListType(f(item))
    @types.FixedListType(spec) =>
      @types.FixedListType({ ..spec, element: f(spec.element), })
    @types.MapType(spec) =>
      @types.MapType({ key: f(spec.key), value: f(spec.value), })
    @types.OptionType(item) => @types.OptionType(f(item))
    @types.ResultType(spec) =>
      @types.ResultType({ ok: spec.ok.map(f), err: spec.err.map(f), })
    @types.UnionType(spec) =>
      @types.UnionType({
        branches: spec.branches.map(branch => {
          ..branch,
          body: f(branch.body),
        }),
      })
    @types.SecretType(spec) =>
      @types.SecretType({ ..spec, inner: f(spec.inner), })
    @types.FutureType(item) => @types.FutureType(item.map(f))
    @types.StreamType(item) => @types.StreamType(item.map(f))
    other => other
  }
}
