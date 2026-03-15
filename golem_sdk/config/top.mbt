///|
pub(all) struct Config[T] {
  value : T
}

///|
pub(all) struct Secret[T] {
  key : Array[String]
  expected_type : @types.WitType
}

///|
pub fn[T : @schema.FromExtractor] Secret::get(
  self : Secret[T],
) -> T raise @common.AgentError {
  let wit_value = @host.get_config_value(self.key, self.expected_type)
  let extractor = @extractor.extract(wit_value)
  T::from_extractor(extractor)
}

///|
fn extract_wit_type(
  schema : @types.ElementSchema,
) -> @types.WitType raise @common.AgentError {
  match schema {
    @types.ElementSchema::ComponentModel(wt) => wt
    _ =>
      raise @common.AgentError::InvalidInput(
        "Config fields must use ComponentModel schema, not unstructured types",
      )
  }
}

///|
pub fn[T : @schema.HasElementSchema] wit_type_of(
  _tag : @schema.TypeTag[T],
) -> @types.WitType raise @common.AgentError {
  extract_wit_type(T::element_schema())
}

///|
pub fn[T : @schema.HasElementSchema + @schema.FromExtractor] load_config_value(
  key : Array[String],
) -> T raise @common.AgentError {
  let expected_type = extract_wit_type(T::element_schema())
  let wit_value = @host.get_config_value(key, expected_type)
  let extractor = @extractor.extract(wit_value)
  T::from_extractor(extractor)
}

///|
pub fn[T : ConfigField] load_config(
  _tag : @schema.TypeTag[T],
) -> Config[T] raise @common.AgentError {
  { value: T::load([]) }
}

///|
pub fn[T : ConfigField] collect_entries_of(
  _tag : @schema.TypeTag[T],
  path : Array[String],
) -> Array[@common.ConfigKeyValueType] raise @common.AgentError {
  T::collect_entries(path)
}

///|
pub fn[T : ConfigField] load_field(
  _tag : @schema.TypeTag[T],
  path : Array[String],
) -> T raise @common.AgentError {
  T::load(path)
}

///|
pub(open) trait ConfigField {
  collect_entries(path : Array[String]) -> Array[@common.ConfigKeyValueType] raise @common.AgentError
  load(path : Array[String]) -> Self raise @common.AgentError
}

///|
fn[T : @schema.HasElementSchema] local_entry(
  _tag : @schema.TypeTag[T],
  path : Array[String],
) -> Array[@common.ConfigKeyValueType] raise @common.AgentError {
  [
    {
      key: path,
      value: @common.ConfigValueType::Local(
        extract_wit_type(T::element_schema()),
      ),
    },
  ]
}

///|
fn[T : @schema.HasElementSchema] shared_entry(
  _tag : @schema.TypeTag[T],
  path : Array[String],
) -> Array[@common.ConfigKeyValueType] raise @common.AgentError {
  [
    {
      key: path,
      value: @common.ConfigValueType::Shared(
        extract_wit_type(T::element_schema()),
      ),
    },
  ]
}

///|
pub impl ConfigField for String with collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[String]), path)
}

///|
pub impl ConfigField for String with load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Bool with collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[Bool]), path)
}

///|
pub impl ConfigField for Bool with load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Int with collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[Int]), path)
}

///|
pub impl ConfigField for Int with load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for UInt with collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[UInt]), path)
}

///|
pub impl ConfigField for UInt with load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Int64 with collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[Int64]), path)
}

///|
pub impl ConfigField for Int64 with load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for UInt64 with collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[UInt64]), path)
}

///|
pub impl ConfigField for UInt64 with load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Float with collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[Float]), path)
}

///|
pub impl ConfigField for Float with load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Double with collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[Double]), path)
}

///|
pub impl ConfigField for Double with load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Byte with collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[Byte]), path)
}

///|
pub impl ConfigField for Byte with load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Char with collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[Char]), path)
}

///|
pub impl ConfigField for Char with load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Bytes with collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[Bytes]), path)
}

///|
pub impl ConfigField for Bytes with load(path) {
  load_config_value(path)
}

///|
pub impl[T : @schema.HasElementSchema + @schema.FromExtractor] ConfigField for T? with collect_entries(
  path,
) {
  let inner_wt = extract_wit_type(T::element_schema())
  let wt = @builder.TypeBuilder::new().option_type(inner_wt)
  [{ key: path, value: @common.ConfigValueType::Local(wt) }]
}

///|
pub impl[T : @schema.HasElementSchema + @schema.FromExtractor] ConfigField for T? with load(
  path,
) {
  load_config_value(path)
}

///|
pub impl[T : @schema.HasElementSchema + @schema.FromExtractor] ConfigField for Array[
  T,
] with collect_entries(path) {
  let inner_wt = extract_wit_type(T::element_schema())
  let wt = @builder.TypeBuilder::new().list_type(inner_wt)
  [{ key: path, value: @common.ConfigValueType::Local(wt) }]
}

///|
pub impl[T : @schema.HasElementSchema + @schema.FromExtractor] ConfigField for Array[
  T,
] with load(path) {
  load_config_value(path)
}

///|
pub impl[
  T : @schema.HasElementSchema + @schema.FromExtractor,
  E : @schema.HasElementSchema + @schema.FromExtractor,
] ConfigField for Result[T, E] with collect_entries(path) {
  let ok_wt = extract_wit_type(T::element_schema())
  let err_wt = extract_wit_type(E::element_schema())
  let wt = @builder.TypeBuilder::new().result_type(ok_wt, err_wt)
  [{ key: path, value: @common.ConfigValueType::Local(wt) }]
}

///|
pub impl[
  T : @schema.HasElementSchema + @schema.FromExtractor,
  E : @schema.HasElementSchema + @schema.FromExtractor,
] ConfigField for Result[T, E] with load(path) {
  load_config_value(path)
}

///|
pub impl[T : @schema.HasElementSchema + @schema.FromExtractor] ConfigField for Secret[
  T,
] with collect_entries(path) {
  shared_entry((@schema.TypeTag::{  } : @schema.TypeTag[T]), path)
}

///|
pub impl[T : @schema.HasElementSchema + @schema.FromExtractor] ConfigField for Secret[
  T,
] with load(path) {
  { key: path, expected_type: extract_wit_type(T::element_schema()) }
}
