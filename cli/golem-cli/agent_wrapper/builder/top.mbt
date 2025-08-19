///|
typealias Int as NodeIndex

///|
pub suberror BuilderError String

///|
struct Builder {
  nodes : Array[@types.WitNode]
}

///|
pub fn Builder::new() -> Builder {
  { nodes: [] }
}

///|
fn Builder::add(self : Builder, node : @types.WitNode) -> NodeIndex {
  self.nodes.push(node)
  self.nodes.length() - 1
}

///|
fn Builder::add_u8(self : Builder, value : Byte) -> NodeIndex {
  self.add(@types.WitNode::PrimU8(value))
}

///|
fn Builder::add_u16(self : Builder, value : UInt) -> NodeIndex {
  self.add(@types.WitNode::PrimU16(value))
}

///|
fn Builder::add_u32(self : Builder, value : UInt) -> NodeIndex {
  self.add(@types.WitNode::PrimU32(value))
}

///|
fn Builder::add_u64(self : Builder, value : UInt64) -> NodeIndex {
  self.add(@types.WitNode::PrimU64(value))
}

///|
fn Builder::add_s8(self : Builder, value : Int) -> NodeIndex {
  self.add(@types.WitNode::PrimS8(value))
}

///|
fn Builder::add_s16(self : Builder, value : Int) -> NodeIndex {
  self.add(@types.WitNode::PrimS16(value))
}

///|
fn Builder::add_s32(self : Builder, value : Int) -> NodeIndex {
  self.add(@types.WitNode::PrimS32(value))
}

///|
fn Builder::add_s64(self : Builder, value : Int64) -> NodeIndex {
  self.add(@types.WitNode::PrimS64(value))
}

///|
fn Builder::add_f32(self : Builder, value : Float) -> NodeIndex {
  self.add(@types.WitNode::PrimFloat32(value))
}

///|
fn Builder::add_f64(self : Builder, value : Double) -> NodeIndex {
  self.add(@types.WitNode::PrimFloat64(value))
}

///|
fn Builder::add_string(self : Builder, value : String) -> NodeIndex {
  self.add(@types.WitNode::PrimString(value))
}

///|
fn Builder::add_char(self : Builder, value : Char) -> NodeIndex {
  self.add(@types.WitNode::PrimChar(value))
}

///|
pub fn Builder::add_bool(self : Builder, value : Bool) -> NodeIndex {
  self.add(@types.WitNode::PrimBool(value))
}

///|
fn Builder::add_record(self : Builder) -> (NodeIndex, ChildItemsBuilder) {
  let idx = self.add(@types.WitNode::RecordValue([]))
  let inner_builder = {
    parent: FinishChildren::{ builder: self, target: idx },
    items: [],
    closed: false,
  }
  (idx, inner_builder)
}

///|
fn Builder::add_variant(
  self : Builder,
  idx : UInt,
  target_idx : Int,
) -> (NodeIndex, ItemBuilder) {
  let variant_idx = self.add(
    @types.WitNode::VariantValue((idx, Some(target_idx))),
  )
  let inner_builder = {
    parent: FinishChild::{ builder: self, target: variant_idx, closed: false },
  }
  (variant_idx, inner_builder)
}

///|
fn Builder::add_variant_unit(self : Builder, idx : UInt) -> NodeIndex {
  self.add(@types.WitNode::VariantValue((idx, None)))
}

///|
fn Builder::add_enum_value(self : Builder, value : UInt) -> NodeIndex {
  self.add(@types.WitNode::EnumValue(value))
}

///|
fn Builder::add_flags(self : Builder, values : Array[Bool]) -> NodeIndex {
  self.add(@types.WitNode::FlagsValue(values))
}

///|
fn Builder::add_tuple(self : Builder) -> (NodeIndex, ChildItemsBuilder) {
  let idx = self.add(@types.WitNode::TupleValue([]))
  let inner_builder = {
    parent: FinishChildren::{ builder: self, target: idx },
    items: [],
    closed: false,
  }
  (idx, inner_builder)
}

///|
fn Builder::add_list(self : Builder) -> (NodeIndex, ChildItemsBuilder) {
  let idx = self.add(@types.WitNode::ListValue([]))
  let inner_builder = {
    parent: FinishChildren::{ builder: self, target: idx },
    items: [],
    closed: false,
  }
  (idx, inner_builder)
}

///|
fn Builder::add_option_none(self : Builder) -> NodeIndex {
  self.add(@types.WitNode::OptionValue(None))
}

///|
fn Builder::add_option_some(self : Builder) -> (NodeIndex, ItemBuilder) {
  let option_idx = self.add(@types.WitNode::OptionValue(Some(-1)))
  let inner_builder = {
    parent: FinishChild::{ builder: self, target: option_idx, closed: false },
  }
  (option_idx, inner_builder)
}

///|
fn Builder::add_result_ok(self : Builder) -> (NodeIndex, ItemBuilder) {
  let result_idx = self.add(@types.WitNode::ResultValue(Ok(Some(-1))))
  let inner_builder = {
    parent: FinishChild::{ builder: self, target: result_idx, closed: false },
  }
  (result_idx, inner_builder)
}

///|
fn Builder::add_result_err(self : Builder) -> (NodeIndex, ItemBuilder) {
  let result_idx = self.add(@types.WitNode::ResultValue(Err(Some(-1))))
  let inner_builder = {
    parent: FinishChild::{ builder: self, target: result_idx, closed: false },
  }
  (result_idx, inner_builder)
}

///|
fn Builder::add_result_ok_unit(self : Builder) -> NodeIndex {
  self.add(@types.WitNode::ResultValue(Ok(None)))
}

///|
fn Builder::add_result_err_unit(self : Builder) -> NodeIndex {
  self.add(@types.WitNode::ResultValue(Err(None)))
}

///|
fn Builder::add_handle(
  self : Builder,
  uri : @types.Uri,
  handle_value : UInt64,
) -> NodeIndex {
  self.add(@types.WitNode::Handle((uri, handle_value)))
}

///|
fn Builder::finish_child(
  self : Builder,
  child : NodeIndex,
  target_idx : NodeIndex,
) -> Unit raise BuilderError {
  match self.nodes[target_idx] {
    OptionValue(result_item) => {
      if result_item.is_empty() {
        raise BuilderError("finish_child called on None option")
      }
      self.nodes[target_idx] = @types.WitNode::OptionValue(Some(child))
    }
    ResultValue(result_item) =>
      match result_item {
        Ok(Some(_)) =>
          self.nodes[target_idx] = @types.WitNode::ResultValue(Ok(Some(child)))
        Ok(None) => raise BuilderError("finish_child called on Ok(None) result")
        Err(Some(_)) =>
          self.nodes[target_idx] = @types.WitNode::ResultValue(Err(Some(child)))
        Err(None) =>
          raise BuilderError("finish_child called on Err(None) result")
      }
    VariantValue((case_idx, result_item)) => {
      if result_item.is_empty() {
        raise BuilderError("finish_child called on variant with no inner value")
      }
      self.nodes[target_idx] = @types.WitNode::VariantValue(
        (case_idx, Some(child)),
      )
    }
    _ =>
      raise BuilderError(
        "finish_child called on a node that is neither Option, Result or Variant",
      )
  }
}

///|
fn Builder::finish_seq(
  self : Builder,
  items : Array[NodeIndex],
  target_idx : NodeIndex,
) -> Unit raise BuilderError {
  match self.nodes[target_idx] {
    RecordValue(result_items) => result_items.append(items)
    TupleValue(result_items) => result_items.append(items)
    ListValue(result_items) => result_items.append(items)
    _ =>
      raise BuilderError(
        "finish_seq called on a node that is neither Record, Tuple or List",
      )
  }
}

///|
pub fn Builder::u8(self : Builder, value : Byte) -> @types.WitValue {
  let _ = self.add_u8(value)
  self.build()
}

///|
pub fn Builder::u16(self : Builder, value : UInt) -> @types.WitValue {
  let _ = self.add_u16(value)
  self.build()
}

///|
pub fn Builder::u32(self : Builder, value : UInt) -> @types.WitValue {
  let _ = self.add_u32(value)
  self.build()
}

///|
pub fn Builder::u64(self : Builder, value : UInt64) -> @types.WitValue {
  let _ = self.add_u64(value)
  self.build()
}

///|
pub fn Builder::s8(self : Builder, value : Int) -> @types.WitValue {
  let _ = self.add_s8(value)
  self.build()
}

///|
pub fn Builder::s16(self : Builder, value : Int) -> @types.WitValue {
  let _ = self.add_s16(value)
  self.build()
}

///|
pub fn Builder::s32(self : Builder, value : Int) -> @types.WitValue {
  let _ = self.add_s32(value)
  self.build()
}

///|
pub fn Builder::s64(self : Builder, value : Int64) -> @types.WitValue {
  let _ = self.add_s64(value)
  self.build()
}

///|
pub fn Builder::f32(self : Builder, value : Float) -> @types.WitValue {
  let _ = self.add_f32(value)
  self.build()
}

///|
pub fn Builder::f64(self : Builder, value : Double) -> @types.WitValue {
  let _ = self.add_f64(value)
  self.build()
}

///|
pub fn Builder::string(self : Builder, value : String) -> @types.WitValue {
  let _ = self.add_string(value)
  self.build()
}

///|
pub fn Builder::char(self : Builder, value : Char) -> @types.WitValue {
  let _ = self.add_char(value)
  self.build()
}

///|
pub fn Builder::bool(self : Builder, value : Bool) -> @types.WitValue {
  let _ = self.add_bool(value)
  self.build()
}

///|
pub fn Builder::enum_value(self : Builder, value : UInt) -> @types.WitValue {
  let _ = self.add_enum_value(value)
  self.build()
}

///|
pub fn Builder::flags(self : Builder, values : Array[Bool]) -> @types.WitValue {
  let _ = self.add_flags(values)
  self.build()
}

///|
pub fn Builder::record(
  self : Builder,
  inner : (ItemBuilder) -> Unit raise BuilderError,
) -> @types.WitValue raise BuilderError {
  let (_, inner_builder) = self.add_record()
  inner_builder.items(inner)
  self.build()
}

///|
pub fn Builder::variant(
  self : Builder,
  case_idx : UInt,
  inner : (ItemBuilder) -> Unit raise BuilderError,
) -> @types.WitValue raise BuilderError {
  let (_, inner_builder) = self.add_variant(case_idx, -1)
  inner(inner_builder)
  self.build()
}

///|
pub fn Builder::variant_unit(
  self : Builder,
  case_idx : UInt,
) -> @types.WitValue {
  let _ = self.add_variant_unit(case_idx)
  self.build()
}

///|
pub fn Builder::variant_option(
  self : Builder,
  case_idx : UInt,
  inner : ((ItemBuilder) -> Unit raise BuilderError)?,
) -> @types.WitValue raise BuilderError {
  let result = match inner {
    Some(inner_fn) => self.variant(case_idx, inner_fn)
    None => self.variant_unit(case_idx)
  }
  result
}

///|
pub fn Builder::tuple(
  self : Builder,
  inner : (ItemBuilder) -> Unit raise BuilderError,
) -> @types.WitValue raise BuilderError {
  let (_, inner_builder) = self.add_tuple()
  inner_builder.items(inner)
  self.build()
}

///|
pub fn Builder::list(
  self : Builder,
  inner : (ItemBuilder) -> Unit raise BuilderError,
) -> @types.WitValue raise BuilderError {
  let (_, inner_builder) = self.add_list()
  inner_builder.items(inner)
  self.build()
}

///|
pub fn Builder::option_some(
  self : Builder,
  inner : (ItemBuilder) -> Unit raise BuilderError,
) -> @types.WitValue raise BuilderError {
  let (_, inner_builder) = self.add_option_some()
  inner(inner_builder)
  self.build()
}

///|
pub fn Builder::option_none(self : Builder) -> @types.WitValue {
  let _ = self.add_option_none()
  self.build()
}

///|
pub fn Builder::option(
  self : Builder,
  inner : ((ItemBuilder) -> Unit raise BuilderError)?,
) -> @types.WitValue raise BuilderError {
  match inner {
    Some(inner_fn) => self.option_some(inner_fn)
    None => self.option_none()
  }
}

///|
pub fn Builder::result_ok(
  self : Builder,
  inner : (ItemBuilder) -> Unit raise BuilderError,
) -> @types.WitValue raise BuilderError {
  let (_, inner_builder) = self.add_result_ok()
  inner(inner_builder)
  self.build()
}

///|
pub fn Builder::result_err(
  self : Builder,
  inner : (ItemBuilder) -> Unit raise BuilderError,
) -> @types.WitValue raise BuilderError {
  let (_, inner_builder) = self.add_result_err()
  inner(inner_builder)
  self.build()
}

///|
pub fn Builder::result_ok_unit(self : Builder) -> @types.WitValue {
  let _ = self.add_result_ok_unit()
  self.build()
}

///|
pub fn Builder::result_err_unit(self : Builder) -> @types.WitValue {
  let _ = self.add_result_err_unit()
  self.build()
}

///|
pub fn Builder::result(
  self : Builder,
  inner : Result[
    ((ItemBuilder) -> Unit raise BuilderError)?,
    ((ItemBuilder) -> Unit raise BuilderError)?,
  ],
) -> @types.WitValue raise BuilderError {
  match inner {
    Ok(Some(inner_fn)) => self.result_ok(inner_fn)
    Ok(None) => self.result_ok_unit()
    Err(Some(inner_fn)) => self.result_err(inner_fn)
    Err(None) => self.result_err_unit()
  }
}

///|
pub fn Builder::handle(
  self : Builder,
  uri : @types.Uri,
  handle_value : UInt64,
) -> @types.WitValue {
  let _ = self.add_handle(uri, handle_value)
  self.build()
}

///|
fn Builder::build(self : Builder) -> @types.WitValue {
  { nodes: self.nodes }
}

///|
priv struct FinishChild {
  builder : Builder
  target : NodeIndex
  mut closed : Bool
}

///|
impl SingleItemBuilderParent for FinishChild with root_builder(
  self : FinishChild,
) {
  self.builder
}

///|
impl SingleItemBuilderParent for FinishChild with add_item(
  self : FinishChild,
  item_idx : NodeIndex,
) -> Unit raise BuilderError {
  self.builder.finish_child(item_idx, self.target)
  self.closed = true
}

///|
impl SingleItemBuilderParent for FinishChild with is_closed(self : FinishChild) -> Bool {
  self.closed
}

///|
priv struct FinishChildren {
  builder : Builder
  target : NodeIndex
}

///|
impl ItemSequenceBuilderParent for FinishChildren with root_builder(
  self : FinishChildren,
) {
  self.builder
}

///|
impl ItemSequenceBuilderParent for FinishChildren with add_items(
  self : FinishChildren,
  items : Array[NodeIndex],
) -> Unit raise BuilderError {
  self.builder.finish_seq(items, self.target)
}

///|
priv trait ItemSequenceBuilderParent {
  root_builder(Self) -> Builder
  add_items(Self, items : Array[NodeIndex]) -> Unit raise BuilderError
}

///|
priv trait SingleItemBuilderParent {
  root_builder(Self) -> Builder
  add_item(Self, item_idx : NodeIndex) -> Unit raise BuilderError
  is_closed(Self) -> Bool
}
