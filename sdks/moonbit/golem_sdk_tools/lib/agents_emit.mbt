///|
/// Converts a `ParamType` to its MoonBit source code string representation.
pub fn param_type_to_type_string(pt : ParamType) -> String {
  match pt {
    Simple(name) => name
    Optional(inner) => "\{param_type_to_type_string(inner)}?"
    List(inner) => "Array[\{param_type_to_type_string(inner)}]"
    ResultType(ok, err) =>
      "Result[\{param_type_to_type_string(ok)}, \{param_type_to_type_string(err)}]"
    Tuple(elems) => {
      let parts = elems.map(param_type_to_type_string)
      "(" + parts.join(", ") + ")"
    }
    MultimodalType(inner) =>
      "@multimodal.Multimodal[\{param_type_to_type_string(inner)}]"
    Parameterized(name, params) => {
      let param_strs = params.map(param_type_to_type_string)
      let joined = param_strs.join(", ")
      "\{name}[\{joined}]"
    }
  }
}

// ============================================================
// Agent registration and dispatch use the flat WIT schema and value carriers.
// Each agent appends type nodes to one shared pool, avoiding recursive models
// and graph merging. Source is parsed back through `parse_generated_impls`.
// ============================================================

///|
/// Lowercases the first character of a name so it can be used as a MoonBit value
/// identifier in the generated `init` block.
fn lower_first(s : String) -> String {
  let chars = s.to_array()
  if chars.length() == 0 {
    return s
  }
  let c = chars[0]
  if c >= 'A' && c <= 'Z' {
    chars[0] = (c.to_int() + 32).unsafe_to_char()
  }
  String::from_array(chars)
}

///|
/// Renders an `Option[String]` as a MoonBit `String?` literal.
fn opt_string_lit(s : String?) -> String {
  match s {
    None => "None"
    Some(v) => "Some(\"\{escape_string_lit(v)}\")"
  }
}

///|
/// Renders the agent execution mode constructor.
fn mode_str(mode : AgentMode) -> String {
  match mode {
    Durable => "@common.AgentMode::DURABLE"
    Ephemeral => "@common.AgentMode::EPHEMERAL"
  }
}

///|
/// Renders the `@common.Snapshotting` value for an agent.
fn snapshotting_str(s : AgentSnapshotting) -> String {
  match s {
    Disabled => "@common.Snapshotting::Disabled"
    Default =>
      "@common.Snapshotting::Enabled(@common.SnapshottingConfig::Default)"
    Periodic(n) =>
      "@common.Snapshotting::Enabled(@common.SnapshottingConfig::Periodic(\{n}UL))"
    EveryNInvocation(n) =>
      "@common.Snapshotting::Enabled(@common.SnapshottingConfig::EveryNInvocation(\{n}U))"
  }
}

///|
/// Renders a method's `read_only` field. `uses_principal` is derived from the
/// method signature (presence of a `Principal` parameter), never user input.
fn read_only_str(info : MethodInfo) -> String {
  match info.read_only {
    None => "None"
    Some(ro) => {
      let cp = match ro.cache_policy {
        NoCache => "@common.CachePolicy::NoCache"
        UntilWrite => "@common.CachePolicy::UntilWrite"
        Ttl(n) => "@common.CachePolicy::Ttl(\{n}UL)"
      }
      let uses_principal = info.params
        .iter()
        .any(fn(p) { is_principal_type(p.1) })
      // Parenthesized type-annotated record (not `@common.ReadOnlyConfig::{...}`):
      // the formatter (0.1.5) crashes when a call's trailing argument is a
      // qualified struct-constructor literal, so we wrap a plain record literal.
      "Some(({ cache_policy: \{cp}, uses_principal: \{uses_principal} } : @common.ReadOnlyConfig))"
    }
  }
}

///|
/// Renders the source and wire type index of a single input parameter.
fn input_param_source_str(pt : ParamType) -> String {
  if is_principal_type(pt) {
    "@common.FieldSource::AutoInjected(@common.AutoInjectedKind::PRINCIPAL), schema: schema_encoder.push(@types.RecordType([]))"
  } else {
    let ts = param_type_to_type_string(pt)
    "@common.FieldSource::UserSupplied, schema: @schema.append_wire_type_with(schema_encoder, (@schema.TypeTag::{  } : @schema.TypeTag[\{ts}]))"
  }
}

///|
/// Emits the `InputParam` array entries for a constructor/method, in declared
/// order. Config parameters are excluded (they become `config` declarations).
fn emit_input_params(
  buf : StringBuilder,
  params : Array[(String, ParamType)],
  indent : String,
) -> Unit {
  for p in params {
    let (name, pt) = p
    if !is_config_type(pt) {
      buf.write_string(
        "\{indent}{ name: \"\{escape_string_lit(name)}\", source: \{input_param_source_str(pt)}, metadata: @schema.empty_wire_metadata() },\n",
      )
    }
  }
}

///|
/// Renders a method output's index in the shared wire type pool.
fn output_schema_str(return_type : ParamType?) -> String {
  match return_type {
    None => "@common.OutputSchema::Unit"
    Some(pt) => {
      let ts = param_type_to_type_string(pt)
      "@common.OutputSchema::Single(@schema.append_wire_type_with(schema_encoder, (@schema.TypeTag::{  } : @schema.TypeTag[\{ts}])))"
    }
  }
}

///|
/// Renders an agent's `config` declarations. Built from the `Config[T]`
/// constructor parameter's inner type, if any.
fn config_entries_str(agent : AgentInfo) -> String {
  match agent.config_type {
    None => "[]"
    Some(ct) =>
      "@config.append_wire_entries_with(schema_encoder, (@schema.TypeTag::{  } : @schema.TypeTag[\{ct}]), [])"
  }
}

///|
/// Emits the wire `AgentType` literal bound to `var_name`.
fn emit_agent_type_def(
  buf : StringBuilder,
  agent : AgentInfo,
  var_name : String,
) -> Unit {
  buf.write_string("    let schema_encoder = @schema.WireTypeEncoder::new()\n")
  buf.write_string(
    "    let schema_root = schema_encoder.push(@types.RecordType([]))\n",
  )
  buf.write_string("    let \{var_name} : @common.AgentType = {\n")
  buf.write_string("      type_name: \"\{escape_string_lit(agent.name)}\",\n")
  let kind = if agent.is_http_router() { "HTTP_ROUTER" } else { "REGULAR" }
  buf.write_string("      kind: @common.AgentTypeKind::\{kind},\n")
  buf.write_string(
    "      description: \"\{escape_string_lit(agent.description)}\",\n",
  )
  buf.write_string("      source_language: \"moonbit\",\n")
  buf.write_string("      constructor_: {\n")
  buf.write_string("        name: Some(\"new\"),\n")
  buf.write_string(
    "        description: \"\{escape_string_lit(agent.constructor_description)}\",\n",
  )
  buf.write_string(
    "        prompt_hint: \{opt_string_lit(agent.constructor_prompt_hint)},\n",
  )
  buf.write_string("        input_schema: @common.InputSchema::Parameters([\n")
  emit_input_params(buf, agent.constructor_params, "          ")
  buf.write_string("        ]),\n")
  buf.write_string("      },\n")
  buf.write_string("      methods: [\n")
  for m in agent.methods {
    buf.write_string("        {\n")
    buf.write_string("          name: \"\{escape_string_lit(m.name)}\",\n")
    buf.write_string(
      "          description: \"\{escape_string_lit(m.description)}\",\n",
    )
    buf.write_string(
      "          http_endpoint: \{http_endpoint_str(m.http_endpoint)},\n",
    )
    buf.write_string(
      "          prompt_hint: \{opt_string_lit(m.prompt_hint)},\n",
    )
    buf.write_string(
      "          input_schema: @common.InputSchema::Parameters([\n",
    )
    emit_input_params(buf, m.params, "            ")
    buf.write_string("          ]),\n")
    buf.write_string(
      "          output_schema: \{output_schema_str(m.return_type)},\n",
    )
    buf.write_string("          read_only: \{read_only_str(m)},\n")
    buf.write_string("        },\n")
  }
  buf.write_string("      ],\n")
  buf.write_string("      dependencies: [],\n")
  buf.write_string("      mode: \{mode_str(agent.mode)},\n")
  buf.write_string("      http_mount: \{http_mount_str(agent.http_mount)},\n")
  buf.write_string(
    "      snapshotting: \{snapshotting_str(agent.snapshotting)},\n",
  )
  buf.write_string("      config: \{config_entries_str(agent)},\n")
  buf.write_string(
    "      schema: { type_nodes: schema_encoder.nodes, defs: schema_encoder.defs, root: schema_root },\n",
  )
  buf.write_string("    }\n")
}

///|
/// Emits a complete constructed-agent bundle for a local `agent` value.
fn emit_constructed_agent(
  buf : StringBuilder,
  indent : String,
  snapshot_format : String?,
) -> Unit {
  let (snapshottable, format) = match snapshot_format {
    None => ("None", "None")
    Some(format) => ("Some(agent)", "Some(@agents.SnapshotFormat::\{format})")
  }
  // Bind the ConstructedAgent to a local before `Ok(...)`. The MoonBit
  // formatter crashes when a call's sole trailing argument is a qualified
  // struct-constructor literal.
  buf.write_string(
    "\{indent}let constructed = @agents.ConstructedAgent::{ raw: agent, snapshottable: \{snapshottable}, snapshot_format: \{format} }\n",
  )
  buf.write_string("\{indent}Ok(constructed)\n")
}

///|
/// Emits the constructor closure for an agent's `register_agent` call.
fn emit_construct(buf : StringBuilder, agent : AgentInfo) -> Unit {
  let user_params = visible_params(agent.constructor_params)
  let arity = user_params.length()
  let ctx = escape_string_lit("\{agent.name}::new")
  buf.write_string("      construct: async fn(input, principal) {\n")
  buf.write_string("        let _ = principal\n")
  buf.write_string("        try {\n")
  buf.write_string(
    "          let params = @agents.decode_wire_input(input, \{arity}, \"\{ctx}\")\n          defer params.abort()\n",
  )
  for j, p in user_params {
    let (name, pt) = p
    buf.write_string(
      "          let \{name} : \{param_type_to_type_string(pt)} = @agents.decode_wire_param(params, \{j}, \"\{ctx}\")\n",
    )
  }
  match find_config_param(agent.constructor_params) {
    Some((cname, ctype, _)) =>
      buf.write_string(
        "          let \{cname} : @config.Config[\{ctype}] = @config.load_config((@schema.TypeTag::{  } : @schema.TypeTag[\{ctype}]))\n",
      )
    None => ()
  }
  let args = agent.constructor_params
    .map(fn(p) { if is_principal_type(p.1) { "principal" } else { p.0 } })
    .join(", ")
  buf.write_string("          params.finish()\n")
  buf.write_string("          let agent = \{agent.name}::new(\{args})\n")
  let use_auto_json = agent.has_json_derives &&
    !agent.has_snapshottable &&
    !agent.is_http_router()
  let format = if agent.has_snapshottable {
    Some("Binary")
  } else if use_auto_json {
    Some("Json")
  } else {
    None
  }
  emit_constructed_agent(buf, "          ", format)
  buf.write_string("        } catch {\n")
  for
    variant in [
      "InvalidInput", "InvalidMethod", "InvalidType", "InvalidAgentId", "CustomError",
    ] {
    buf.write_string(
      "          @common.AgentError::\{variant}(value) => Err(@common.AgentError::\{variant}(value))\n",
    )
  }
  buf.write_string("          e => raise e\n")
  buf.write_string("        }\n")
  buf.write_string("      },\n")
}

///|
/// Emits the snapshot restoration factory for an agent's registration.
fn emit_load_snapshot(buf : StringBuilder, agent : AgentInfo) -> Unit {
  let use_auto_json = agent.has_json_derives &&
    !agent.has_snapshottable &&
    !agent.is_http_router()
  if !agent.has_snapshottable && !use_auto_json {
    buf.write_string("      load_snapshot: None,\n")
    return
  }
  buf.write_string("      load_snapshot: Some(async fn(bytes, context) {\n")
  if agent.has_snapshottable {
    buf.write_string(
      "        let agent = match \{agent.name}::load_snapshot(bytes, context) {\n",
    )
    buf.write_string("          Ok(agent) => agent\n")
    buf.write_string("          Err(e) => return Err(e)\n")
    buf.write_string("        }\n")
    emit_constructed_agent(buf, "        ", Some("Binary"))
  } else {
    buf.write_string("        let _ = context\n")
    buf.write_string("        let json_str = @utf8.decode_lossy(bytes[:])\n")
    buf.write_string("        let json = @json.parse(json_str) catch {\n")
    buf.write_string(
      "          e => return Err(\"Failed to parse snapshot JSON: \" + e.to_string())\n",
    )
    buf.write_string("        }\n")
    buf.write_string(
      "        let agent : \{agent.name} = @json.from_json(json) catch {\n",
    )
    buf.write_string(
      "          e => return Err(\"Failed to deserialize snapshot: \" + e.to_string())\n",
    )
    buf.write_string("        }\n")
    emit_constructed_agent(buf, "        ", Some("Json"))
  }
  buf.write_string("      }),\n")
}

///|
/// Emits a single agent's wire descriptor and
/// `@agents.register_agent({...})` block.
fn emit_register(buf : StringBuilder, agent : AgentInfo) -> Unit {
  let var_name = lower_first(agent.name) + "_type_def"
  emit_agent_type_def(buf, agent, var_name)
  buf.write_string("    @agents.register_agent({\n")
  buf.write_string("      name: \"\{escape_string_lit(agent.name)}\",\n")
  buf.write_string("      agent_type: \{var_name},\n")
  emit_construct(buf, agent)
  emit_load_snapshot(buf, agent)
  buf.write_string("    })\n")
}

///|
/// Emits the `init` block that registers every agent.
fn emit_init(buf : StringBuilder, agents : Array[AgentInfo]) -> Unit {
  buf.write_string("///|\n")
  buf.write_string("fn init {\n")
  for a in agents {
    emit_register(buf, a)
  }
  buf.write_string("}\n\n")
}

///|
/// Emits a method dispatch match arm for the `RawAgent::invoke` body.
fn emit_method_arm(
  buf : StringBuilder,
  agent : AgentInfo,
  m : MethodInfo,
) -> Unit {
  let user_params = visible_params(m.params)
  let arity = user_params.length()
  let ctx = escape_string_lit("\{agent.name}.\{m.name}")
  buf.write_string("      \"\{escape_string_lit(m.name)}\" => {\n")
  buf.write_string(
    "        let params = @agents.decode_wire_input(input, \{arity}, \"\{ctx}\")\n        defer params.abort()\n",
  )
  for j, p in user_params {
    let (name, pt) = p
    buf.write_string(
      "        let \{name} : \{param_type_to_type_string(pt)} = @agents.decode_wire_param(params, \{j}, \"\{ctx}\")\n",
    )
  }
  buf.write_string("        params.finish()\n")
  let args = m.params
    .map(fn(p) { if is_principal_type(p.1) { "principal" } else { p.0 } })
    .join(", ")
  match m.return_type {
    None => {
      buf.write_string("        self.\{m.name}(\{args})\n")
      buf.write_string("        Ok(None)\n")
    }
    Some(_) => {
      buf.write_string("        let result = self.\{m.name}(\{args})\n")
      buf.write_string("        Ok(Some(@agents.encode_wire_output(result)))\n")
    }
  }
  buf.write_string("      }\n")
}

///|
/// Emits the `impl @agents.RawAgent for <Name>` dispatch block.
fn emit_raw_agent(buf : StringBuilder, agent : AgentInfo) -> Unit {
  buf.write_string("///|\n#warnings(\"-partial_match\")\n")
  buf.write_string(
    "impl @agents.RawAgent for \{agent.name} with invoke(self, method_name, input, principal) {\n",
  )
  buf.write_string("  let _ = principal\n")
  buf.write_string("  try {\n")
  buf.write_string("    match method_name {\n")
  for m in agent.methods {
    emit_method_arm(buf, agent, m)
  }
  buf.write_string(
    "      other => {\n        @agents.discard_wire_value(input)\n        Err(@common.AgentError::InvalidMethod(other))\n      }\n",
  )
  buf.write_string("    }\n")
  buf.write_string("  } catch {\n")
  buf.write_string(
    "    @common.AgentError::InvalidInput(message) => Err(@common.AgentError::InvalidInput(message))\n",
  )
  buf.write_string(
    "    @common.AgentError::InvalidMethod(method_name) => Err(@common.AgentError::InvalidMethod(method_name))\n",
  )
  buf.write_string(
    "    @common.AgentError::InvalidType(message) => Err(@common.AgentError::InvalidType(message))\n",
  )
  buf.write_string(
    "    @common.AgentError::InvalidAgentId(message) => Err(@common.AgentError::InvalidAgentId(message))\n",
  )
  buf.write_string(
    "    @common.AgentError::CustomError(value) => Err(@common.AgentError::CustomError(value))\n",
  )
  buf.write_string("  }\n")
  buf.write_string("}\n\n")
}

///|
/// Emits the instance-based snapshot saver for an agent that derives JSON
/// (`ToJson` + `FromJson`) and has no hand-written `Snapshottable`.
fn emit_snapshottable(buf : StringBuilder, agent : AgentInfo) -> Unit {
  if !agent.has_json_derives ||
    agent.has_snapshottable ||
    agent.is_http_router() {
    return
  }
  let name = agent.name
  buf.write_string("///|\n")
  buf.write_string(
    "impl @agents.Snapshottable for \{name} with save_snapshot(self) -> Bytes {\n",
  )
  buf.write_string("  @utf8.encode(ToJson::to_json(self).stringify()[:])\n")
  buf.write_string("}\n\n")
}

///|
/// Generates all agent registration code: the `init` block calling
/// `register_agent`, the `RawAgent` dispatch impls, and snapshot savers for
/// JSON-deriving agents.
pub fn generate_agent_registration(
  agents : Array[AgentInfo],
) -> @list.List[@syntax.Impl] {
  let buf = StringBuilder()
  emit_init(buf, agents)
  for a in agents {
    emit_raw_agent(buf, a)
  }
  for a in agents {
    emit_snapshottable(buf, a)
  }
  parse_generated_impls(buf.to_string(), "agents")
}
