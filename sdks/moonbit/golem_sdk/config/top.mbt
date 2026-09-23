// Configuration loading uses direct wire type and value codecs. Recursive
// schema declarations remain available for explicit dynamic consumers.

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
#warnings("-unused_type_variable")
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
    e => raise @common.AgentError::InvalidInput(repr(e))
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
    root: @model.schema_type(Secret({ inner: inner.root, category: None, })),
  }
}

///|
/// Load and decode a configuration value of type `T` from the given key path.
pub fn[T : @schema.WireSchema + @schema.FromWire] load_config_value(
  key : Array[String],
) -> T raise @common.AgentError {
  let wire_graph = @schema.wire_schema(
    (@schema.type_tag() : @schema.TypeTag[T]),
  )
  let wire_value = match @host.get_config_value(key, wire_graph) {
    Ok(value) => value
    Err(@host.ConfigValueError::PermissionDenied) =>
      raise @common.AgentError::InvalidInput("config access denied")
  }
  @schema.from_wire(wire_value) catch {
    e => raise @common.AgentError::InvalidInput(repr(e))
  }
}

///|
/// Read the secret value, building its schema graph from `T`.
pub fn[T : @schema.WireSchema + @schema.FromWire] Secret::get(
  self : Secret[T],
) -> T raise @common.AgentError {
  let tag = (@schema.TypeTag::{ } : @schema.TypeTag[T])
  let inner_wire_graph = @schema.wire_schema(tag)
  let nodes = inner_wire_graph.type_nodes.copy()
  let secret_root = nodes.length()
  nodes.push({
    body: @types.SecretType({ inner: inner_wire_graph.root, category: None, }),
    metadata: @schema.empty_wire_metadata(),
  })
  let secret_wire_graph : @types.SchemaGraph = {
    type_nodes: nodes,
    defs: inner_wire_graph.defs,
    root: secret_root,
  }
  let secret_wire_value = match
    @host.get_config_value(self.key, secret_wire_graph) {
    Ok(value) => value
    Err(@host.ConfigValueError::PermissionDenied) =>
      raise @common.AgentError::InvalidInput("config access denied")
  }
  let handle : @model.GuestSecretHandle = @schema.from_wire(secret_wire_value) catch {
    e => raise @common.AgentError::InvalidInput(repr(e))
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
      let msg = Repr(e)
      raise @common.AgentError::InvalidInput("secret reveal failed: \{msg}")
    }
  }
  @schema.from_wire(revealed_wire_value) catch {
    e => raise @common.AgentError::InvalidInput(repr(e))
  }
}

///|
/// Materialize a `Config[T]` (loads every field from the empty root path).
pub fn[T : ConfigField] load_config(
  _tag : @schema.TypeTag[T],
) -> Config[T] raise @common.AgentError {
  { value: T::load([]), }
}

///|
/// Loads fresh agent configuration while constructing an agent from a snapshot.
pub fn[T : ConfigField] load_snapshot_config(
  context : @agents.SnapshotRestoreContext,
  tag : @schema.TypeTag[T],
) -> Config[T] raise @common.AgentError {
  let _ = context
  load_config(tag)
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
  local_entry((@schema.TypeTag::{ } : @schema.TypeTag[String]), path)
}

///|
pub impl ConfigField for String with fn load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Bool with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{ } : @schema.TypeTag[Bool]), path)
}

///|
pub impl ConfigField for Bool with fn load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Int with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{ } : @schema.TypeTag[Int]), path)
}

///|
pub impl ConfigField for Int with fn load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for UInt with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{ } : @schema.TypeTag[UInt]), path)
}

///|
pub impl ConfigField for UInt with fn load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Int64 with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{ } : @schema.TypeTag[Int64]), path)
}

///|
pub impl ConfigField for Int64 with fn load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for UInt64 with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{ } : @schema.TypeTag[UInt64]), path)
}

///|
pub impl ConfigField for UInt64 with fn load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Float with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{ } : @schema.TypeTag[Float]), path)
}

///|
pub impl ConfigField for Float with fn load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Double with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{ } : @schema.TypeTag[Double]), path)
}

///|
pub impl ConfigField for Double with fn load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Byte with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{ } : @schema.TypeTag[Byte]), path)
}

///|
pub impl ConfigField for Byte with fn load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Char with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{ } : @schema.TypeTag[Char]), path)
}

///|
pub impl ConfigField for Char with fn load(path) {
  load_config_value(path)
}

///|
pub impl ConfigField for Bytes with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{ } : @schema.TypeTag[Bytes]), path)
}

///|
pub impl ConfigField for Bytes with fn load(path) {
  load_config_value(path)
}

///|
pub impl[T : @schema.IntoSchema + @schema.WireSchema + @schema.FromWire] ConfigField for T? with fn collect_entries(
  path,
) {
  local_entry((@schema.TypeTag::{ } : @schema.TypeTag[T?]), path)
}

///|
pub impl[T : @schema.IntoSchema + @schema.WireSchema + @schema.FromWire] ConfigField for T? with fn load(
  path,
) {
  load_config_value(path)
}

///|
pub impl[T : @schema.IntoSchema + @schema.WireSchema + @schema.FromWire] ConfigField for Array[
  T,
] with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{ } : @schema.TypeTag[Array[T]]), path)
}

///|
pub impl[T : @schema.IntoSchema + @schema.WireSchema + @schema.FromWire] ConfigField for Array[
  T,
] with fn load(path) {
  load_config_value(path)
}

///|
pub impl[
  T : @schema.IntoSchema + @schema.WireSchema + @schema.FromWire,
  E : @schema.IntoSchema + @schema.WireSchema + @schema.FromWire,
] ConfigField for Result[T, E] with fn collect_entries(path) {
  local_entry((@schema.TypeTag::{ } : @schema.TypeTag[Result[T, E]]), path)
}

///|
pub impl[
  T : @schema.IntoSchema + @schema.WireSchema + @schema.FromWire,
  E : @schema.IntoSchema + @schema.WireSchema + @schema.FromWire,
] ConfigField for Result[T, E] with fn load(path) {
  load_config_value(path)
}

///|
pub impl[T : @schema.IntoSchema] ConfigField for Secret[T] with fn collect_entries(
  path,
) {
  shared_entry((@schema.TypeTag::{ } : @schema.TypeTag[T]), path)
}

///|
pub impl[T : @schema.IntoSchema] ConfigField for Secret[T] with fn load(path) {
  { key: path, }
}
