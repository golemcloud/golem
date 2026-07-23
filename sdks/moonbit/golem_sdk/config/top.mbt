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
/// Build the config lookup graph for `Secret[T]`: the host returns an opaque
/// secret handle whose declared inner payload is `T`'s schema graph.
fn[T : @schema.IntoSchema] secret_schema_graph_of(
  tag : @schema.TypeTag[T],
) -> @model.SchemaGraph raise @common.AgentError {
  let inner = schema_graph_of(tag)
  {
    defs: inner.defs,
    root: @model.schema_type(Secret({ inner: inner.root, category: None })),
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
  let tag = (@schema.TypeTag::{  } : @schema.TypeTag[T])
  let inner_graph = schema_graph_of(tag)
  let secret_graph = secret_schema_graph_of(tag)
  let secret_wire_graph = @model.schema_graph_to_wit(secret_graph) catch {
    e => raise @common.AgentError::InvalidInput(e.to_string())
  }
  let secret_wire_value = @host.get_config_value(self.key, secret_wire_graph)
  let secret_model_value = @model.schema_value_from_wit(secret_wire_value) catch {
    e => raise @common.AgentError::InvalidInput(e.to_string())
  }
  let handle = match secret_model_value {
    Secret(handle) => handle
    other =>
      raise @common.AgentError::InvalidInput(
        "expected secret handle from config, got \{@schema.value_kind(other)}",
      )
  }
  let inner_wire_graph = @model.schema_graph_to_wit(inner_graph) catch {
    e => raise @common.AgentError::InvalidInput(e.to_string())
  }
  let raw_handle = match handle.take() {
    Some(h) => h
    None =>
      raise @common.AgentError::InvalidInput(
        "secret handle has already been transferred and can no longer be revealed",
      )
  }
  let reveal_result = @reveal.reveal(raw_handle, inner_wire_graph)
  raw_handle.drop()
  let revealed_wire_value = match reveal_result {
    Ok(v) => v
    Err(e) => {
      let msg = e.to_string()
      raise @common.AgentError::InvalidInput("secret reveal failed: \{msg}")
    }
  }
  let revealed_model_value = @model.schema_value_from_wit(revealed_wire_value) catch {
    e => raise @common.AgentError::InvalidInput(e.to_string())
  }
  let decoded : T = @schema.from_value_as(revealed_model_value) catch {
    e => raise @common.AgentError::InvalidInput(e.to_string())
  }
  decoded
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
  fn collect_entries(path : Array[String]) -> Array[@agents.AgentConfigDef] raise @common.AgentError
  fn load(path : Array[String]) -> Self raise @common.AgentError
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
/// Single `SECRET` config declaration whose value type is `Secret[T]` with
/// inner payload `T`'s schema graph.
fn[T : @schema.IntoSchema] shared_entry(
  tag : @schema.TypeTag[T],
  path : Array[String],
) -> Array[@agents.AgentConfigDef] raise @common.AgentError {
  [
    {
      source: @common.AgentConfigSource::SECRET,
      path,
      value_type: secret_schema_graph_of(tag),
    },
  ]
}

///|
pub impl ConfigField for String with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[String]), path)
}

///|
pub impl ConfigField for String with fn load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Bool with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[Bool]), path)
}

///|
pub impl ConfigField for Bool with fn load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Int with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[Int]), path)
}

///|
pub impl ConfigField for Int with fn load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for UInt with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[UInt]), path)
}

///|
pub impl ConfigField for UInt with fn load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Int64 with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[Int64]), path)
}

///|
pub impl ConfigField for Int64 with fn load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for UInt64 with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[UInt64]), path)
}

///|
pub impl ConfigField for UInt64 with fn load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Float with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[Float]), path)
}

///|
pub impl ConfigField for Float with fn load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Double with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[Double]), path)
}

///|
pub impl ConfigField for Double with fn load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Byte with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[Byte]), path)
}

///|
pub impl ConfigField for Byte with fn load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Char with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[Char]), path)
}

///|
pub impl ConfigField for Char with fn load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Bytes with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[Bytes]), path)
}

///|
pub impl ConfigField for Bytes with fn load(path) {
  load_config_value(path)
}

///|
pub impl[T : @schema.IntoSchema + @schema.FromSchema] ConfigField for T? with fn collect_entries(
  path,
) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[T?]), path)
}

///|
pub impl[T : @schema.IntoSchema + @schema.FromSchema] ConfigField for T? with fn load(
  path,
) {
  load_config_value(path)
}

///|
pub impl[T : @schema.IntoSchema + @schema.FromSchema] ConfigField for Array[T] with fn collect_entries(
  path,
) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[Array[T]]), path)
}

///|
pub impl[T : @schema.IntoSchema + @schema.FromSchema] ConfigField for Array[T] with fn load(
  path,
) {
  load_config_value(path)
}

///|
pub impl[
  T : @schema.IntoSchema + @schema.FromSchema,
  E : @schema.IntoSchema + @schema.FromSchema,
] ConfigField for Result[T, E] with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{  } : @schema.TypeTag[Result[T, E]]), path)
}

///|
pub impl[
  T : @schema.IntoSchema + @schema.FromSchema,
  E : @schema.IntoSchema + @schema.FromSchema,
] ConfigField for Result[T, E] with fn load(path) {
  load_config_value(path)
}

///|
pub impl[T : @schema.IntoSchema + @schema.FromSchema] ConfigField for Secret[T] with fn collect_entries(
  path,
) {
  shared_entry((@schema.TypeTag::{  } : @schema.TypeTag[T]), path)
}

///|
pub impl[T : @schema.IntoSchema + @schema.FromSchema] ConfigField for Secret[T] with fn load(
  path,
) {
  { key: path }
}
