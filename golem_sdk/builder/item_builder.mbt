///|
struct ItemBuilder {
  parent : &SingleItemBuilderParent
}

///|
fn ItemBuilder::root_builder(self : ItemBuilder) -> Builder {
  self.parent.root_builder()
}

///|
pub fn ItemBuilder::u8(
  self : ItemBuilder,
  value : Byte,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let item_type_index = self.root_builder().add_u8(value)
  self.parent.add_item(item_type_index)
}

///|
pub fn ItemBuilder::u16(
  self : ItemBuilder,
  value : UInt,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let item_type_index = self.root_builder().add_u16(value)
  self.parent.add_item(item_type_index)
}

///|
pub fn ItemBuilder::u32(
  self : ItemBuilder,
  value : UInt,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let item_type_index = self.root_builder().add_u32(value)
  self.parent.add_item(item_type_index)
}

///|
pub fn ItemBuilder::u64(
  self : ItemBuilder,
  value : UInt64,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let item_type_index = self.root_builder().add_u64(value)
  self.parent.add_item(item_type_index)
}

///|
pub fn ItemBuilder::s8(
  self : ItemBuilder,
  value : Int,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let item_type_index = self.root_builder().add_s8(value)
  self.parent.add_item(item_type_index)
}

///|
pub fn ItemBuilder::s16(
  self : ItemBuilder,
  value : Int,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let item_type_index = self.root_builder().add_s16(value)
  self.parent.add_item(item_type_index)
}

///|
pub fn ItemBuilder::s32(
  self : ItemBuilder,
  value : Int,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let item_type_index = self.root_builder().add_s32(value)
  self.parent.add_item(item_type_index)
}

///|
pub fn ItemBuilder::s64(
  self : ItemBuilder,
  value : Int64,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let item_type_index = self.root_builder().add_s64(value)
  self.parent.add_item(item_type_index)
}

///|
pub fn ItemBuilder::f32(
  self : ItemBuilder,
  value : Float,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let item_type_index = self.root_builder().add_f32(value)
  self.parent.add_item(item_type_index)
}

///|
pub fn ItemBuilder::f64(
  self : ItemBuilder,
  value : Double,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let item_type_index = self.root_builder().add_f64(value)
  self.parent.add_item(item_type_index)
}

///|
pub fn ItemBuilder::string(
  self : ItemBuilder,
  value : String,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let item_type_index = self.root_builder().add_string(value)
  self.parent.add_item(item_type_index)
}

///|
pub fn ItemBuilder::char(
  self : ItemBuilder,
  value : Char,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let item_type_index = self.root_builder().add_char(value)
  self.parent.add_item(item_type_index)
}

///|
pub fn ItemBuilder::bool(
  self : ItemBuilder,
  value : Bool,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let item_type_index = self.root_builder().add_bool(value)
  self.parent.add_item(item_type_index)
}

///|
pub fn ItemBuilder::enum_value(
  self : ItemBuilder,
  value : UInt,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let item_type_index = self.root_builder().add_enum_value(value)
  self.parent.add_item(item_type_index)
}

///|
pub fn ItemBuilder::flags(
  self : ItemBuilder,
  values : Array[Bool],
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let item_type_index = self.root_builder().add_flags(values)
  self.parent.add_item(item_type_index)
}

///|
pub fn ItemBuilder::record(
  self : ItemBuilder,
  inner : (ItemBuilder) -> Unit raise BuilderError,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let (item_type_index, inner_builder) = self.root_builder().add_record()
  self.parent.add_item(item_type_index)
  inner_builder.items(inner)
}

///|
pub fn ItemBuilder::variant(
  self : ItemBuilder,
  idx : UInt,
  inner : (ItemBuilder) -> Unit raise BuilderError,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let (item_type_index, inner_builder) = self
    .root_builder()
    .add_variant(idx, -1)
  self.parent.add_item(item_type_index)
  inner(inner_builder)
}

///|
pub fn ItemBuilder::variant_unit(
  self : ItemBuilder,
  idx : UInt,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let item_type_index = self.root_builder().add_variant_unit(idx)
  self.parent.add_item(item_type_index)
}

///|
pub fn ItemBuilder::variant_option(
  self : ItemBuilder,
  idx : UInt,
  inner : ((ItemBuilder) -> Unit raise BuilderError)?,
) -> Unit raise BuilderError {
  match inner {
    Some(inner_fn) => self.variant(idx, inner_fn)
    None => self.variant_unit(idx)
  }
}

///|
pub fn ItemBuilder::tuple(
  self : ItemBuilder,
  inner : (ItemBuilder) -> Unit raise BuilderError,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let (item_type_index, inner_builder) = self.root_builder().add_tuple()
  self.parent.add_item(item_type_index)
  inner_builder.items(inner)
}

///|
pub fn ItemBuilder::list(
  self : ItemBuilder,
  inner : (ItemBuilder) -> Unit raise BuilderError,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let (item_type_index, inner_builder) = self.root_builder().add_list()
  self.parent.add_item(item_type_index)
  inner_builder.items(inner)
}

///|
pub fn ItemBuilder::option_none(self : ItemBuilder) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let item_type_index = self.root_builder().add_option_none()
  self.parent.add_item(item_type_index)
}

///|
pub fn ItemBuilder::option_some(
  self : ItemBuilder,
  inner : (ItemBuilder) -> Unit raise BuilderError,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let (item_type_index, inner_builder) = self.root_builder().add_option_some()
  self.parent.add_item(item_type_index)
  inner(inner_builder)
}

///|
pub fn ItemBuilder::option(
  self : ItemBuilder,
  inner : ((ItemBuilder) -> Unit raise BuilderError)?,
) -> Unit raise BuilderError {
  match inner {
    Some(inner_fn) => self.option_some(inner_fn)
    None => self.option_none()
  }
}

///|
pub fn ItemBuilder::result_ok(
  self : ItemBuilder,
  inner : (ItemBuilder) -> Unit raise BuilderError,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let (item_type_index, inner_builder) = self.root_builder().add_result_ok()
  self.parent.add_item(item_type_index)
  inner(inner_builder)
}

///|
pub fn ItemBuilder::result_err(
  self : ItemBuilder,
  inner : (ItemBuilder) -> Unit raise BuilderError,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let (item_type_index, inner_builder) = self.root_builder().add_result_err()
  self.parent.add_item(item_type_index)
  inner(inner_builder)
}

///|
pub fn ItemBuilder::result_ok_unit(
  self : ItemBuilder,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let item_type_index = self.root_builder().add_result_ok_unit()
  self.parent.add_item(item_type_index)
}

///|
pub fn ItemBuilder::result_err_unit(
  self : ItemBuilder,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let item_type_index = self.root_builder().add_result_err_unit()
  self.parent.add_item(item_type_index)
}

///|
pub fn ItemBuilder::result(
  self : ItemBuilder,
  inner : Result[
    ((ItemBuilder) -> Unit raise BuilderError)?,
    ((ItemBuilder) -> Unit raise BuilderError)?,
  ],
) -> Unit raise BuilderError {
  match inner {
    Ok(Some(inner_fn)) => self.result_ok(inner_fn)
    Ok(None) => self.result_ok_unit()
    Err(Some(inner_fn)) => self.result_err(inner_fn)
    Err(None) => self.result_err_unit()
  }
}

///|
pub fn ItemBuilder::handle(
  self : ItemBuilder,
  uri : @types.Uri,
  handle_value : UInt64,
) -> Unit raise BuilderError {
  if self.parent.is_closed() {
    raise BuilderError("ItemBuilder is already closed")
  }
  let item_type_index = self.root_builder().add_handle(uri, handle_value)
  self.parent.add_item(item_type_index)
}
