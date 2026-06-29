// Agent-type assembly: turn an SDK-side "definition" (per-parameter / per-output
// recursive `@model.SchemaGraph`s) into a wire `@common.AgentType` whose schema
// roots are `type-node-index` values into a single shared, deduplicated
// `@types.SchemaGraph` pool.
//
// This mirrors the Rust SDK's `ExtendedAgentType::to_agent_type` /
// `build_agent_schema` (`sdks/rust/golem-rust/src/agentic/extended_agent_type.rs`):
// generated code (Phase 4) builds the `AgentTypeDef` from each parameter's
// `into_schema_graph::<T>()` graph; the runtime merges every graph into one
// def pool, then encodes each root (constructor inputs, method inputs/outputs,
// config value types) into that pool with a shared `@model.GraphEncoder`.
//
// `schema.root` of the produced pool is a structural placeholder (an empty
// record produced by `GraphEncoder::finish`); the semantic roots are the
// `type-node-index` values installed into `NamedField.schema`,
// `OutputSchema::Single`, and `AgentConfigDeclaration.value_type`.

///|
/// How an input parameter's value is provided. `UserSupplied` carries the
/// parameter's own schema graph; `AutoInjected` is filled in by the platform
/// (e.g. the principal) and contributes no graph of its own — it encodes as an
/// empty record placeholder, matching the Rust SDK.
pub(all) enum InputParamSource {
  UserSupplied(@model.SchemaGraph)
  AutoInjected(@common.AutoInjectedKind)
}

///|
/// A single named input parameter of a constructor or method.
pub(all) struct InputParam {
  name : String
  source : InputParamSource
}

///|
/// SDK-side constructor definition (pre-encoding).
pub(all) struct AgentConstructorDef {
  name : String?
  description : String
  prompt_hint : String?
  input : Array[InputParam]
}

///|
/// SDK-side method definition (pre-encoding). `output` is `None` for a unit
/// return and `Some(graph)` for a single returned value.
pub(all) struct AgentMethodDef {
  name : String
  description : String
  http_endpoint : Array[@common.HttpEndpointDetails]
  prompt_hint : String?
  input : Array[InputParam]
  output : @model.SchemaGraph?
  read_only : @common.ReadOnlyConfig?
}

///|
/// SDK-side config declaration (pre-encoding).
pub(all) struct AgentConfigDef {
  source : @common.AgentConfigSource
  path : Array[String]
  value_type : @model.SchemaGraph
}

///|
/// SDK-side agent type definition (pre-encoding). Generated code constructs this
/// and calls `to_agent_type` to obtain the wire `@common.AgentType`.
pub(all) struct AgentTypeDef {
  type_name : String
  description : String
  source_language : String
  constructor_ : AgentConstructorDef
  methods : Array[AgentMethodDef]
  dependencies : Array[@common.AgentDependency]
  mode : @common.AgentMode
  http_mount : @common.HttpMountDetails?
  snapshotting : @common.Snapshotting
  config : Array[AgentConfigDef]
}

///|
/// Collect every `@model.SchemaGraph` an agent type contributes, in a stable
/// order (constructor inputs, then per method: inputs then output, then config).
/// Auto-injected parameters contribute no graph. The collected graphs feed
/// `@model.merge_graph_defs`, which deduplicates their named definitions by
/// `TypeId`.
fn AgentTypeDef::collect_graphs(
  self : AgentTypeDef,
) -> Array[@model.SchemaGraph] {
  let graphs : Array[@model.SchemaGraph] = []
  collect_input_graphs(self.constructor_.input, graphs)
  for method in self.methods {
    collect_input_graphs(method.input, graphs)
    if method.output is Some(g) {
      graphs.push(g)
    }
  }
  for config in self.config {
    graphs.push(config.value_type)
  }
  graphs
}

///|
fn collect_input_graphs(
  params : Array[InputParam],
  graphs : Array[@model.SchemaGraph],
) -> Unit {
  for param in params {
    if param.source is UserSupplied(g) {
      graphs.push(g)
    }
  }
}

///|
/// Encode an input parameter list into an `@common.InputSchema`, installing each
/// parameter's `type-node-index` into the shared pool via `encoder`.
fn encode_input_schema(
  encoder : @model.GraphEncoder,
  params : Array[InputParam],
) -> @common.InputSchema raise @model.SchemaModelError {
  let fields : Array[@common.NamedField] = []
  for param in params {
    let (source, schema) = match param.source {
      UserSupplied(g) =>
        (@common.FieldSource::UserSupplied, encoder.encode_type(g.root))
      AutoInjected(kind) =>
        (
          @common.FieldSource::AutoInjected(kind),
          encoder.encode_type(@model.schema_type(Record([]))),
        )
    }
    fields.push({
      name: param.name,
      source,
      schema,
      metadata: @model.empty_metadata(),
    })
  }
  @common.InputSchema::Parameters(fields)
}

///|
/// Encode a method's output into an `@common.OutputSchema`.
fn encode_output_schema(
  encoder : @model.GraphEncoder,
  output : @model.SchemaGraph?,
) -> @common.OutputSchema raise @model.SchemaModelError {
  match output {
    None => @common.OutputSchema::Unit
    Some(g) => @common.OutputSchema::Single(encoder.encode_type(g.root))
  }
}

///|
/// Build the wire `@common.AgentType` from this definition.
///
/// All constructor / method / config schema graphs are merged into one shared
/// def pool (deduplicated by `TypeId`; conflicting same-id bodies raise
/// `@model.SchemaModelError`), then every root is encoded into that pool. The
/// resulting indices are installed as `NamedField.schema` /
/// `OutputSchema::Single` / `AgentConfigDeclaration.value_type`, and the encoded
/// pool becomes `AgentType.schema`.
pub fn AgentTypeDef::to_agent_type(
  self : AgentTypeDef,
) -> @common.AgentType raise @model.SchemaModelError {
  let defs = @model.merge_graph_defs(self.collect_graphs())
  let encoder = @model.GraphEncoder::new(defs)
  let constructor_input = encode_input_schema(encoder, self.constructor_.input)
  let methods : Array[@common.AgentMethod] = []
  for method in self.methods {
    let input_schema = encode_input_schema(encoder, method.input)
    let output_schema = encode_output_schema(encoder, method.output)
    methods.push({
      name: method.name,
      description: method.description,
      http_endpoint: method.http_endpoint,
      prompt_hint: method.prompt_hint,
      input_schema,
      output_schema,
      read_only: method.read_only,
    })
  }
  let config : Array[@common.AgentConfigDeclaration] = []
  for decl in self.config {
    config.push({
      source: decl.source,
      path: decl.path,
      value_type: encoder.encode_type(decl.value_type.root),
    })
  }
  let schema = encoder.finish()
  {
    type_name: self.type_name,
    description: self.description,
    source_language: self.source_language,
    schema,
    constructor_: {
      name: self.constructor_.name,
      description: self.constructor_.description,
      prompt_hint: self.constructor_.prompt_hint,
      input_schema: constructor_input,
    },
    methods,
    dependencies: self.dependencies,
    mode: self.mode,
    http_mount: self.http_mount,
    snapshotting: self.snapshotting,
    config,
  }
}
