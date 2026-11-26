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
      let reference = encoder.named(
        def.id,
        () => append_config_type(encoder, graph, def.body),
        name=def.name,
      )
      encoder.nodes[reference] = {
        ..encoder.nodes[reference],
        metadata: node.metadata,
      }
      reference
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
