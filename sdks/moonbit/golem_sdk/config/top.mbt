// Agent configuration over the schema model.
//
// Mirrors the Rust SDK's `agentic::agent_config` (`Config<T>`, `Secret<T>`,
// `ConfigSchema`) and `extended_agent_type` config handling:
//   * runtime loading uses the new host import
//     `get_config_value(key, expected : @types.SchemaGraph) -> @types.SchemaValueTree`,
//     decoded via `FromSchema` (build the expected graph from `IntoSchema`,
//     encode it to the wire form, call the host, decode the returned value).
//   * config *declarations* are produced in the pre-encoding form
//     `@agents.AgentConfigDef { source, path, value_type : @model.SchemaGraph }`
//     (the MoonBit analogue of Rust `ExtendedAgentConfigDeclaration`); the
//     agent-type assembler (`agents/agent_type.mbt`) merges each `value_type`
//     graph into the agent's shared schema pool and installs a `type-node-index`
//     into the wire `@common.AgentConfigDeclaration`.

///|
/// A loaded configuration value of type `T`. Generated constructor glue
/// materializes `Config[T]` parameters via `load_config`.
pub(all) struct Config[T] {
  value : T
}

///|
/// A handle to a secret configuration value. Carries only the config key path;
/// the schema graph is derived from `T` on demand in `get` (matching the Rust
/// `Secret<T>` phantom design).
pub(all) struct Secret[T] {
  key : Array[String]
}

///|
/// Build the `@model.SchemaGraph` for `T`, mapping schema construction /
/// validation failures to `@common.AgentError::InvalidInput`.
fn[T : @schema.IntoSchema] schema_graph_of(
  tag : @schema.TypeTag[T],
) -> @model.SchemaGraph raise @common.AgentError {
  @schema.into_schema_graph(tag) catch {
    e => raise @common.AgentError::InvalidInput(e.to_string())
  }
}

///|
/// Load and decode a configuration value of type `T` from the given key path.
///
/// Pipeline (mirrors Rust `Secret::get` / `ConfigSchema::load` leaf):
/// build `T`'s schema graph → encode to the wire `@types.SchemaGraph` →
/// `get_config_value` → decode the returned `@types.SchemaValueTree` to a
/// model `SchemaValue` → `FromSchema`.
pub fn[T : @schema.IntoSchema + @schema.FromSchema] load_config_value(
  key : Array[String],
) -> T raise @common.AgentError {
  let graph = schema_graph_of((@schema.TypeTag::{  } : @schema.TypeTag[T]))
  let wire_graph = @model.schema_graph_to_wit(graph) catch {
    e => raise @common.AgentError::InvalidInput(e.to_string())
  }
  let wire_value = @host.get_config_value(key, wire_graph)
  let model_value = @model.schema_value_from_wit(wire_value) catch {
    e => raise @common.AgentError::InvalidInput(e.to_string())
  }
  let decoded : T = @schema.from_value_as(model_value) catch {
    e => raise @common.AgentError::InvalidInput(e.to_string())
  }
  decoded
}

///|
/// Read the secret value, building its schema graph from `T`.
pub fn[T : @schema.IntoSchema + @schema.FromSchema] Secret::get(
  self : Secret[T],
) -> T raise @common.AgentError {
  load_config_value(self.key)
}

///|
/// Materialize a `Config[T]` (loads every field from the empty root path).
pub fn[T : ConfigField] load_config(
  _tag : @schema.TypeTag[T],
) -> Config[T] raise @common.AgentError {
  { value: T::load([]) }
}

///|
/// Collect the config declarations contributed by `T` rooted at `path`.
pub fn[T : ConfigField] collect_entries_of(
  _tag : @schema.TypeTag[T],
  path : Array[String],
) -> Array[@agents.AgentConfigDef] raise @common.AgentError {
  T::collect_entries(path)
}

///|
/// Load a single config field of type `T` at `path`.
pub fn[T : ConfigField] load_field(
  _tag : @schema.TypeTag[T],
  path : Array[String],
) -> T raise @common.AgentError {
  T::load(path)
}

///|
/// A type usable as an agent configuration field. Generated code implements this
/// for `#derive.config` structs; the SDK provides impls for primitives, the
/// standard compounds, and `Secret[T]`.
pub(open) trait ConfigField {
  collect_entries(path : Array[String]) -> Array[@agents.AgentConfigDef] raise @common.AgentError
  load(path : Array[String]) -> Self raise @common.AgentError
}

///|
/// Single `LOCAL` config declaration whose value type is `T`'s schema graph.
fn[T : @schema.IntoSchema] local_entry(
  tag : @schema.TypeTag[T],
  path : Array[String],
) -> Array[@agents.AgentConfigDef] raise @common.AgentError {
  [
    {
      source: @common.AgentConfigSource::LOCAL,
      path,
      value_type: schema_graph_of(tag),
    },
  ]
}

///|
/// Single `SECRET` config declaration whose value type is `T`'s schema graph.
fn[T : @schema.IntoSchema] shared_entry(
  tag : @schema.TypeTag[T],
  path : Array[String],
) -> Array[@agents.AgentConfigDef] raise @common.AgentError {
  [
    {
      source: @common.AgentConfigSource::SECRET,
      path,
      value_type: schema_graph_of(tag),
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
pub impl[T : @schema.IntoSchema + @schema.FromSchema] ConfigField for T? with collect_entries(
  path,
) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[T?]), path)
}

///|
pub impl[T : @schema.IntoSchema + @schema.FromSchema] ConfigField for T? with load(
  path,
) {
  load_config_value(path)
}

///|
pub impl[T : @schema.IntoSchema + @schema.FromSchema] ConfigField for Array[T] with collect_entries(
  path,
) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[Array[T]]), path)
}

///|
pub impl[T : @schema.IntoSchema + @schema.FromSchema] ConfigField for Array[T] with load(
  path,
) {
  load_config_value(path)
}

///|
pub impl[
  T : @schema.IntoSchema + @schema.FromSchema,
  E : @schema.IntoSchema + @schema.FromSchema,
] ConfigField for Result[T, E] with collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[Result[T, E]]), path)
}

///|
pub impl[
  T : @schema.IntoSchema + @schema.FromSchema,
  E : @schema.IntoSchema + @schema.FromSchema,
] ConfigField for Result[T, E] with load(path) {
  load_config_value(path)
}

///|
pub impl[T : @schema.IntoSchema + @schema.FromSchema] ConfigField for Secret[T] with collect_entries(
  path,
) {
  shared_entry((@schema.TypeTag::{  } : @schema.TypeTag[T]), path)
}

///|
pub impl[T : @schema.IntoSchema + @schema.FromSchema] ConfigField for Secret[T] with load(
  path,
) {
  { key: path }
}
