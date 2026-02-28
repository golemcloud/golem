///|
fn merge_wit_type(
  target : Array[@types.NamedWitTypeNode],
  source : @types.WitType,
) -> Int {
  let offset = target.length()
  for node in source.nodes {
    target.push({
      name: node.name,
      owner: node.owner,
      type_: rebase_type(node.type_, offset),
    })
  }
  offset
}

///|
fn rebase_type(node : @types.WitTypeNode, offset : Int) -> @types.WitTypeNode {
  match node {
    RecordType(fields) =>
      @types.WitTypeNode::RecordType(fields.map(fn(f) { (f.0, f.1 + offset) }))
    VariantType(cases) =>
      @types.WitTypeNode::VariantType(
        cases.map(fn(c) { (c.0, c.1.map(fn(i) { i + offset })) }),
      )
    TupleType(elems) =>
      @types.WitTypeNode::TupleType(elems.map(fn(i) { i + offset }))
    ListType(i) => @types.WitTypeNode::ListType(i + offset)
    OptionType(i) => @types.WitTypeNode::OptionType(i + offset)
    ResultType((ok, err)) =>
      @types.WitTypeNode::ResultType(
        (ok.map(fn(i) { i + offset }), err.map(fn(i) { i + offset })),
      )
    _ => node
  }
}
