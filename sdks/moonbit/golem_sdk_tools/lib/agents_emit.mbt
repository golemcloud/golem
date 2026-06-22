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
// Agent registration + dispatch emission (new schema model)
//
// Generates `golem_agents.mbt` for the recursive schema model
// (`golem:core/types@2.0.0`): the `init` block builds an
// `@agents.AgentTypeDef` per agent (per-parameter `@schema.into_schema_graph`
// roots), registers it, and the generated `@agents.RawAgent` dispatch decodes
// the `@types.SchemaValueTree` input record and encodes the output via the SDK
// runtime helpers in `agents/dispatch.mbt`. Built as a MoonBit-source string and
// parsed back via `parse_generated_impls` (see `value_types_emit.mbt`).
// ============================================================

///|
/// Lowercases the first character of a name so it can be used as a MoonBit value
/// identifier (the local `AgentTypeDef` binding in the generated `init` block).
fn lower_first(s : String) -> String {
  let chars = s.to_array()
  if chars.length() == 0 {
    return s
  }
  let c = chars[0]
  if c >= 'A' && c <= 'Z' {
    chars[0] = Char::from_int(c.to_int() + 32)
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
/// Renders the `source` of a single input parameter. A `Principal` parameter is
/// auto-injected by the platform; every other parameter carries its own schema
/// graph (built from `T`'s `@schema.into_schema_graph`).
fn input_param_source_str(pt : ParamType) -> String {
  if is_principal_type(pt) {
    "@agents.InputParamSource::AutoInjected(@common.AutoInjectedKind::PRINCIPAL)"
  } else {
    let ts = param_type_to_type_string(pt)
    "@agents.InputParamSource::UserSupplied(@schema.into_schema_graph((@schema.TypeTag::{  } : @schema.TypeTag[\{ts}])))"
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
    if not(is_config_type(pt)) {
      buf.write_string(
        "\{indent}{ name: \"\{escape_string_lit(name)}\", source: \{input_param_source_str(pt)} },\n",
      )
    }
  }
}

///|
/// Renders a method/constructor `output` schema (`None` for a unit return).
fn output_schema_str(return_type : ParamType?) -> String {
  match return_type {
    None => "None"
    Some(pt) => {
      let ts = param_type_to_type_string(pt)
      "Some(@schema.into_schema_graph((@schema.TypeTag::{  } : @schema.TypeTag[\{ts}])))"
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
      "@config.collect_entries_of((@schema.TypeTag::{  } : @schema.TypeTag[\{ct}]), [])"
  }
}

///|
/// Emits the `AgentTypeDef` literal bound to `var_name`.
fn emit_agent_type_def(
  buf : StringBuilder,
  agent : AgentInfo,
  var_name : String,
) -> Unit {
  buf.write_string("    let \{var_name} : @agents.AgentTypeDef = {\n")
  buf.write_string("      type_name: \"\{escape_string_lit(agent.name)}\",\n")
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
  buf.write_string("        input: [\n")
  emit_input_params(buf, agent.constructor_params, "          ")
  buf.write_string("        ],\n")
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
    buf.write_string("          input: [\n")
    emit_input_params(buf, m.params, "            ")
    buf.write_string("          ],\n")
    buf.write_string("          output: \{output_schema_str(m.return_type)},\n")
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
  buf.write_string("    }\n")
}

///|
/// Emits the constructor closure for an agent's `register_agent` call.
fn emit_construct(buf : StringBuilder, agent : AgentInfo) -> Unit {
  let user_params = visible_params(agent.constructor_params)
  let arity = user_params.length()
  let ctx = escape_string_lit("\{agent.name}::new")
  buf.write_string("      construct: fn(input, principal) {\n")
  buf.write_string("        let _ = principal\n")
  buf.write_string("        try {\n")
  if arity > 0 {
    buf.write_string(
      "          let params = @agents.decode_invocation_input(input, \{arity}, \"\{ctx}\")\n",
    )
    for j, p in user_params {
      let (name, pt) = p
      buf.write_string(
        "          let \{name} : \{param_type_to_type_string(pt)} = @agents.decode_param(params, \{j}, \"\{ctx}\")\n",
      )
    }
  } else {
    buf.write_string(
      "          let _ = @agents.decode_invocation_input(input, 0, \"\{ctx}\")\n",
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
  buf.write_string("          let agent = \{agent.name}::new(\{args})\n")
  let use_auto_json = agent.has_json_derives && not(agent.has_snapshottable)
  let (snap, fmt) = if agent.has_snapshottable || use_auto_json {
    let f = if agent.has_snapshottable { "Binary" } else { "Json" }
    ("Some(agent)", "Some(@agents.SnapshotFormat::\{f})")
  } else {
    ("None", "None")
  }
  // NOTE: bind the ConstructedAgent to a local before `Ok(...)`. The MoonBit
  // formatter (0.1.5) crashes when a call's sole trailing argument is a
  // qualified struct-constructor literal (`Ok(@pkg.Type::{...})`), so we emit
  // `let constructed = @agents.ConstructedAgent::{...}; Ok(constructed)`.
  buf.write_string(
    "          let constructed = @agents.ConstructedAgent::{ raw: agent, snapshottable: \{snap}, snapshot_format: \{fmt} }\n",
  )
  buf.write_string("          Ok(constructed)\n")
  buf.write_string("        } catch {\n")
  buf.write_string("          e => Err(e)\n")
  buf.write_string("        }\n")
  buf.write_string("      },\n")
}

///|
/// Emits a single agent's `let <var> : AgentTypeDef = {...}` +
/// `@agents.register_agent({...})` block inside the `init` try.
fn emit_register(buf : StringBuilder, agent : AgentInfo) -> Unit {
  let var_name = lower_first(agent.name) + "_type_def"
  emit_agent_type_def(buf, agent, var_name)
  buf.write_string("    @agents.register_agent({\n")
  buf.write_string("      name: \"\{escape_string_lit(agent.name)}\",\n")
  buf.write_string("      agent_type: \{var_name}.to_agent_type(),\n")
  emit_construct(buf, agent)
  buf.write_string("    })\n")
}

///|
/// Emits the `init` block that registers every agent. The whole body is wrapped
/// in a `try/catch` because `to_agent_type()` (and `collect_entries_of`) raise.
fn emit_init(buf : StringBuilder, agents : Array[AgentInfo]) -> Unit {
  buf.write_string("///|\n")
  buf.write_string("fn init {\n")
  buf.write_string("  try {\n")
  for a in agents {
    emit_register(buf, a)
  }
  buf.write_string("  } catch {\n")
  // Use string concatenation, not `\{e}` interpolation: the formatter (0.1.5)
  // drops interpolation segments when round-tripping generated source.
  buf.write_string(
    "    e =>\n      @agents.panic_with_message(\"init\", \"Failed to register agents: \" + e.to_string())\n",
  )
  buf.write_string("  }\n")
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
  if arity > 0 {
    buf.write_string(
      "        let params = @agents.decode_invocation_input(input, \{arity}, \"\{ctx}\")\n",
    )
    for j, p in user_params {
      let (name, pt) = p
      buf.write_string(
        "        let \{name} : \{param_type_to_type_string(pt)} = @agents.decode_param(params, \{j}, \"\{ctx}\")\n",
      )
    }
  } else {
    buf.write_string(
      "        let _ = @agents.decode_invocation_input(input, 0, \"\{ctx}\")\n",
    )
  }
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
      buf.write_string("        Ok(Some(@agents.encode_output(result)))\n")
    }
  }
  buf.write_string("      }\n")
}

///|
/// Emits the `impl @agents.RawAgent for <Name>` dispatch block.
fn emit_raw_agent(buf : StringBuilder, agent : AgentInfo) -> Unit {
  buf.write_string("///|\n")
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
    "      other => Err(@common.AgentError::InvalidMethod(other))\n",
  )
  buf.write_string("    }\n")
  buf.write_string("  } catch {\n")
  buf.write_string("    e => Err(e)\n")
  buf.write_string("  }\n")
  buf.write_string("}\n\n")
}

///|
/// Emits a direct `@agents.Snapshottable` impl for an agent that derives JSON
/// (`ToJson` + `FromJson`) and has no hand-written `Snapshottable`. Uses
/// UTF-8-safe byte conversion for the JSON snapshot envelope.
fn emit_snapshottable(buf : StringBuilder, agent : AgentInfo) -> Unit {
  if not(agent.has_json_derives) || agent.has_snapshottable {
    return
  }
  let name = agent.name
  buf.write_string("///|\n")
  buf.write_string(
    "impl @agents.Snapshottable for \{name} with save_snapshot(self) -> Bytes {\n",
  )
  buf.write_string("  @utf8.encode(self.to_json().stringify()[:])\n")
  buf.write_string("}\n\n")
  buf.write_string("///|\n")
  buf.write_string(
    "impl @agents.Snapshottable for \{name} with load_snapshot(self, bytes) -> Result[\n",
  )
  buf.write_string("  Unit,\n  String,\n] {\n")
  buf.write_string("  let json_str = @utf8.decode_lossy(bytes[:])\n")
  buf.write_string("  let json = @json.parse(json_str) catch {\n")
  buf.write_string(
    "    e => return Err(\"Failed to parse snapshot JSON: \" + e.to_string())\n",
  )
  buf.write_string("  }\n")
  buf.write_string("  let new_val : \{name} = @json.from_json(json) catch {\n")
  buf.write_string(
    "    e => return Err(\"Failed to deserialize snapshot: \" + e.to_string())\n",
  )
  buf.write_string("  }\n")
  for f in agent.fields {
    let (fname, is_mut) = f
    if is_mut {
      buf.write_string("  self.\{fname} = new_val.\{fname}\n")
    }
  }
  buf.write_string("  Ok(())\n")
  buf.write_string("}\n\n")
}

///|
/// Generates all agent registration code: the `init` block calling
/// `register_agent`, the `RawAgent` dispatch impls, and `Snapshottable` impls
/// for JSON-deriving agents.
pub fn generate_agent_registration(
  agents : Array[AgentInfo],
) -> @list.List[@syntax.Impl] {
  let buf = StringBuilder::new()
  emit_init(buf, agents)
  for a in agents {
    emit_raw_agent(buf, a)
  }
  for a in agents {
    emit_snapshottable(buf, a)
  }
  parse_generated_impls(buf.to_string(), "agents")
}
