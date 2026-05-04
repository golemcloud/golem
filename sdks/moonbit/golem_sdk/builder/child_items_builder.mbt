///|
/// Builder for adding multiple child items within a container node (record, tuple, list).
/// Collects child indices and finalizes them on the parent.
struct ChildItemsBuilder {
  parent : &ItemSequenceBuilderParent
  items : Array[NodeIndex]
  mut closed : Bool
}

///|
/// Executes the callback to add child items, then finalizes the sequence on the parent node.
pub fn ChildItemsBuilder::items(
  self : ChildItemsBuilder,
  inner : (ItemBuilder) -> Unit raise BuilderError,
) -> Unit raise BuilderError {
  let item_builder = { parent: self }
  inner(item_builder)
  self.closed = true
  self.parent.add_items(self.items)
}

///|
impl SingleItemBuilderParent for ChildItemsBuilder with add_item(
  self : ChildItemsBuilder,
  item_type_index : NodeIndex,
) -> Unit {
  self.items.push(item_type_index)
}

///|
impl SingleItemBuilderParent for ChildItemsBuilder with root_builder(
  self : ChildItemsBuilder,
) -> Builder {
  self.parent.root_builder()
}

///|
impl SingleItemBuilderParent for ChildItemsBuilder with is_closed(
  self : ChildItemsBuilder,
) -> Bool {
  self.closed
}
