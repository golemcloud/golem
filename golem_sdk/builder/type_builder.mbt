///|
/// Fluent builder for constructing `WitType` trees (type descriptions for schemas).
pub struct TypeBuilder {
  nodes : Array[@types.NamedWitTypeNode]
}

///|
/// Creates a new empty `WitType` builder.
pub fn TypeBuilder::new() -> TypeBuilder {
  { nodes: [] }
}

///|
/// Adds an unnamed type node and returns its index.
fn TypeBuilder::add(self : TypeBuilder, type_node : @types.WitTypeNode) -> Int {
  self.nodes.push({ name: None, owner: None, type_: type_node })
  self.nodes.length() - 1
}

///|
/// Appends an existing `WitType`'s nodes into this builder, rebasing indices. Returns the root index.
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
/// Consumes the builder and returns the constructed `WitType`.
fn TypeBuilder::build(self : TypeBuilder) -> @types.WitType {
  { nodes: self.nodes }
}

///|
/// Adjusts all child indices in a `WitTypeNode` by the given offset, used when composing sub-trees.
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
/// Builds a `WitType` describing an optional type wrapping the given inner type.
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
/// Builds a `WitType` describing a list type with the given element type.
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
/// Builds a `WitType` describing a result type with the given ok and error types.
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
/// Builds a `WitType` describing a record type with the given named fields.
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
/// Builds a `WitType` describing a variant type with the given named cases and optional payloads.
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
/// Builds a `WitType` describing a tuple type with the given element types.
pub fn TypeBuilder::tuple_type(
  self : TypeBuilder,
  elements : Array[@types.WitType],
) -> @types.WitType {
  let idx = self.add(@types.WitTypeNode::TupleType([]))
  let elem_indices : Array[Int] = []
  for i in 0..<elements.length() {
    let root = self.add_wit_type(elements[i])
    elem_indices.push(root)
  }
  self.nodes[idx] = {
    name: None,
    owner: None,
    type_: @types.WitTypeNode::TupleType(elem_indices),
  }
  self.build()
}

///|
/// Builds a `WitType` describing an enum type with the given case names.
pub fn TypeBuilder::enum_type(
  self : TypeBuilder,
  cases : Array[String],
) -> @types.WitType {
  let _ = self.add(@types.WitTypeNode::EnumType(cases))
  self.build()
}
