///|
pub struct TypeBuilder {
  nodes : Array[@types.NamedWitTypeNode]
}

///|
pub fn TypeBuilder::new() -> TypeBuilder {
  { nodes: [] }
}

///|
fn TypeBuilder::add(self : TypeBuilder, type_node : @types.WitTypeNode) -> Int {
  self.nodes.push({ name: None, owner: None, type_: type_node })
  self.nodes.length() - 1
}

///|
pub fn TypeBuilder::add_wit_type(
  self : TypeBuilder,
  wit_type : @types.WitType,
) -> Int {
  let offset = self.nodes.length()
  for node in wit_type.nodes {
    self.nodes.push({
      name: node.name,
      owner: node.owner,
      type_: rebase_type_node(node.type_, offset),
    })
  }
  offset
}

///|
fn TypeBuilder::build(self : TypeBuilder) -> @types.WitType {
  { nodes: self.nodes }
}

///|
fn rebase_type_node(
  node : @types.WitTypeNode,
  offset : Int,
) -> @types.WitTypeNode {
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

///|
pub fn TypeBuilder::option_type(
  self : TypeBuilder,
  inner : @types.WitType,
) -> @types.WitType {
  let idx = self.add(@types.WitTypeNode::OptionType(0))
  let inner_root = self.add_wit_type(inner)
  self.nodes[idx] = {
    name: None,
    owner: None,
    type_: @types.WitTypeNode::OptionType(inner_root),
  }
  self.build()
}

///|
pub fn TypeBuilder::list_type(
  self : TypeBuilder,
  inner : @types.WitType,
) -> @types.WitType {
  let idx = self.add(@types.WitTypeNode::ListType(0))
  let inner_root = self.add_wit_type(inner)
  self.nodes[idx] = {
    name: None,
    owner: None,
    type_: @types.WitTypeNode::ListType(inner_root),
  }
  self.build()
}

///|
pub fn TypeBuilder::result_type(
  self : TypeBuilder,
  ok : @types.WitType,
  err : @types.WitType,
) -> @types.WitType {
  let idx = self.add(@types.WitTypeNode::ResultType((None, None)))
  let ok_root = self.add_wit_type(ok)
  let err_root = self.add_wit_type(err)
  self.nodes[idx] = {
    name: None,
    owner: None,
    type_: @types.WitTypeNode::ResultType((Some(ok_root), Some(err_root))),
  }
  self.build()
}

///|
pub fn TypeBuilder::record_type(
  self : TypeBuilder,
  fields : Array[(String, @types.WitType)],
) -> @types.WitType {
  let idx = self.add(@types.WitTypeNode::RecordType([]))
  let field_entries : Array[(String, Int)] = []
  for i in 0..<fields.length() {
    let (name, wit_type) = fields[i]
    let root = self.add_wit_type(wit_type)
    field_entries.push((name, root))
  }
  self.nodes[idx] = {
    name: None,
    owner: None,
    type_: @types.WitTypeNode::RecordType(field_entries),
  }
  self.build()
}

///|
pub fn TypeBuilder::variant_type(
  self : TypeBuilder,
  cases : Array[(String, @types.WitType?)],
) -> @types.WitType {
  let idx = self.add(@types.WitTypeNode::VariantType([]))
  let case_entries : Array[(String, Int?)] = []
  for i in 0..<cases.length() {
    let (name, payload) = cases[i]
    match payload {
      None => case_entries.push((name, None))
      Some(wit_type) => {
        let root = self.add_wit_type(wit_type)
        case_entries.push((name, Some(root)))
      }
    }
  }
  self.nodes[idx] = {
    name: None,
    owner: None,
    type_: @types.WitTypeNode::VariantType(case_entries),
  }
  self.build()
}

///|
pub fn TypeBuilder::enum_type(
  self : TypeBuilder,
  cases : Array[String],
) -> @types.WitType {
  let _ = self.add(@types.WitTypeNode::EnumType(cases))
  self.build()
}
