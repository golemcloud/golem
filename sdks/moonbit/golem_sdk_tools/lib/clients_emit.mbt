// Emission of typed RPC client stubs (`golem_clients.mbt`) for the recursive
// schema model (`golem:core/types@2.0.0`).
//
// Each agent gets a `<Name>Client` wrapping `@rpc.AgentClient`. Constructor and
// method arguments are encoded into a `@types.SchemaValueTree` input record via
// `@agents.encode_invocation_input`, and awaited results decoded via
// `@agents.decode_output`. Every generated method raises `@common.AgentError`
// (the encode helper raises), matching the SDK's uniform error contract.
//
// Built as a MoonBit-source string and parsed back via `parse_generated_impls`
// (see `value_types_emit.mbt` / `agents_emit.mbt`).

///|
/// Checks if any constructor parameter uses an RPC-incompatible type.
fn has_rpc_incompatible_constructor(agent : AgentInfo) -> Bool {
  visible_params(agent.constructor_params)
  .iter()
  .find_first(fn(p) { is_rpc_incompatible_type(p.1) })
  is Some(_)
}

///|
/// Checks if any parameter or return type of a method uses an RPC-incompatible type.
fn method_has_rpc_incompatible_types(meth : MethodInfo) -> Bool {
  for p in visible_params(meth.params) {
    if is_rpc_incompatible_type(p.1) {
      return true
    }
  }
  match meth.return_type {
    Some(pt) => is_rpc_incompatible_type(pt)
    None => false
  }
}

///|
/// Renders the typed parameter signature fragment for a parameter list
/// (e.g. `name : String, count : UInt64`).
fn params_sig(params : Array[(String, ParamType)]) -> String {
  params.map(fn(p) { "\{p.0} : \{param_type_to_type_string(p.1)}" }).join(", ")
}

///|
/// Renders the `@agents.encode_invocation_input([...])` expression that encodes
/// the user-supplied parameters (in declared order) into a wire input record.
fn encode_input_expr(params : Array[(String, ParamType)]) -> String {
  let parts = params.map(fn(p) { "@schema.to_value_as(\{p.0})" }).join(", ")
  "@agents.encode_invocation_input([\{parts}])"
}

///|
/// Generates typed RPC client stubs for all agents.
pub fn generate_client_code(
  agents : Array[AgentInfo],
  config_types? : Array[ConfigTypeInfo] = [],
  agent_package? : String = "",
) -> @list.List[@syntax.Impl] {
  let buf = StringBuilder::new()
  for agent in agents {
    if has_rpc_incompatible_constructor(agent) {
      continue
    }
    emit_client(buf, agent, config_types, agent_package)
  }
  let source = buf.to_string()
  if source.is_empty() {
    return @list.List::from_array([])
  }
  parse_generated_impls(source, "clients")
}

///|
/// Emits the full client surface for a single agent.
fn emit_client(
  buf : StringBuilder,
  agent : AgentInfo,
  config_types : Array[ConfigTypeInfo],
  agent_package : String,
) -> Unit {
  let client = agent.name + "Client"
  let vis = visible_params(agent.constructor_params)
  let ctor_sig = params_sig(vis)
  let ctor_input = encode_input_expr(vis)
  let comma_sig = if ctor_sig.is_empty() { "" } else { ctor_sig + ",\n  " }
  // struct
  buf.write_string("///|\n")
  buf.write_string(
    "pub(all) struct \{client} {\n  client : @rpc.AgentClient\n}\n\n",
  )
  // get (durable only)
  let durable = match agent.mode {
    Durable => true
    Ephemeral => false
  }
  if durable {
    buf.write_string("///|\n")
    buf.write_string(
      "pub fn \{client}::get(\n  \{comma_sig}) -> \{client} raise @common.AgentError {\n",
    )
    buf.write_string("  let constructor_input = \{ctor_input}\n")
    buf.write_string(
      "  let client = @rpc.AgentClient::get(\"\{escape_string_lit(agent.name)}\", constructor_input)\n",
    )
    buf.write_string("  { client, }\n}\n\n")
  }
  // new_phantom
  buf.write_string("///|\n")
  buf.write_string(
    "pub fn \{client}::new_phantom(\n  \{comma_sig}) -> \{client} raise @common.AgentError {\n",
  )
  buf.write_string("  let constructor_input = \{ctor_input}\n")
  buf.write_string(
    "  let client = @rpc.AgentClient::new_phantom(\"\{escape_string_lit(agent.name)}\", constructor_input)\n",
  )
  buf.write_string("  { client, }\n}\n\n")
  // get_phantom
  buf.write_string("///|\n")
  buf.write_string(
    "pub fn \{client}::get_phantom(\n  \{comma_sig}phantom_id : @types.Uuid,\n) -> \{client} raise @common.AgentError {\n",
  )
  buf.write_string("  let constructor_input = \{ctor_input}\n")
  buf.write_string(
    "  let client = @rpc.AgentClient::get_phantom(\"\{escape_string_lit(agent.name)}\", constructor_input, phantom_id)\n",
  )
  buf.write_string("  { client, }\n}\n\n")
  // get_agent_id
  buf.write_string("///|\n")
  buf.write_string(
    "pub fn \{client}::get_agent_id(self : \{client}) -> String raise @common.AgentError {\n  self.client.get_agent_id()\n}\n\n",
  )
  // phantom_id
  buf.write_string("///|\n")
  buf.write_string(
    "pub fn \{client}::phantom_id(self : \{client}) -> @types.Uuid? {\n  self.client.phantom_id()\n}\n\n",
  )
  // drop
  buf.write_string("///|\n")
  buf.write_string(
    "pub fn \{client}::drop(self : \{client}) -> Unit {\n  self.client.drop()\n}\n\n",
  )
  // scoped (durable only)
  if durable {
    emit_client_scoped(buf, client, "scoped", "get", vis, [])
  }
  emit_client_scoped(buf, client, "scoped_new_phantom", "new_phantom", vis, [])
  emit_client_scoped(buf, client, "scoped_get_phantom", "get_phantom", vis, [
    ("phantom_id", "@types.Uuid"),
  ])
  // methods
  for meth in agent.methods {
    if method_has_rpc_incompatible_types(meth) {
      continue
    }
    emit_client_method(buf, agent, meth, client)
  }
  // _with_config variants
  match resolve_config_type(agent, config_types, agent_package) {
    Some(ct) => {
      // Use the same recursive "has overridable leaves" predicate as the
      // config emitter, so we only emit `_with_config` variants when an
      // `*Override` type + walker were actually generated for `ct`.
      let by_name : Map[String, ConfigTypeInfo] = {}
      for c in config_types {
        by_name[c.name] = c
      }
      if config_has_overridable_fields(ct, by_name) {
        let override_name = ct.name + "Override"
        let walker = "collect_" + pascal_to_snake(ct.name) + "_overrides"
        emit_client_config_variants(
          buf, agent, client, vis, override_name, walker, durable,
        )
      }
    }
    None => ()
  }
}

///|
/// Emits a `scoped*` convenience method that acquires a client, defers `drop`,
/// and runs the callback. `extra` are additional parameters (e.g. `phantom_id`)
/// passed through to the underlying constructor.
fn emit_client_scoped(
  buf : StringBuilder,
  client : String,
  fn_name : String,
  ctor_name : String,
  vis : Array[(String, ParamType)],
  extra : Array[(String, String)],
) -> Unit {
  let sig_parts : Array[String] = []
  for p in vis {
    sig_parts.push("\{p.0} : \{param_type_to_type_string(p.1)}")
  }
  for e in extra {
    sig_parts.push("\{e.0} : \{e.1}")
  }
  sig_parts.push("f : (\{client}) -> T raise @common.AgentError")
  let call_args : Array[String] = []
  for p in vis {
    call_args.push(p.0)
  }
  for e in extra {
    call_args.push(e.0)
  }
  let sig = sig_parts.join(",\n  ")
  let call = call_args.join(", ")
  buf.write_string("///|\n")
  buf.write_string(
    "pub fn[T] \{client}::\{fn_name}(\n  \{sig},\n) -> T raise @common.AgentError {\n",
  )
  buf.write_string("  let client = \{client}::\{ctor_name}(\{call})\n")
  buf.write_string("  defer client.drop()\n")
  buf.write_string("  f(client)\n}\n\n")
}

///|
/// Emits the four per-method client functions: awaited invoke, fire-and-forget
/// trigger, scheduled invoke, and cancelable scheduled invoke.
fn emit_client_method(
  buf : StringBuilder,
  agent : AgentInfo,
  meth : MethodInfo,
  client : String,
) -> Unit {
  let vis = visible_params(meth.params)
  let ctx = escape_string_lit("\{agent.name}.\{meth.name}")
  let input_expr = encode_input_expr(vis)
  let arg_sig = params_sig(vis)
  let self_args = if arg_sig.is_empty() {
    "self : \{client}"
  } else {
    "self : \{client}, \{arg_sig}"
  }
  let ret = match meth.return_type {
    None => "Unit"
    Some(pt) => param_type_to_type_string(pt)
  }
  // awaited invoke
  buf.write_string("///|\n")
  buf.write_string(
    "pub fn \{client}::\{meth.name}(\n  \{self_args},\n) -> \{ret} raise @common.AgentError {\n",
  )
  buf.write_string("  let input = \{input_expr}\n")
  match meth.return_type {
    None =>
      buf.write_string(
        "  let _ = self.client.invoke_and_await(\"\{escape_string_lit(meth.name)}\", input)\n",
      )
    Some(_) => {
      buf.write_string(
        "  let result = self.client.invoke_and_await(\"\{escape_string_lit(meth.name)}\", input)\n",
      )
      buf.write_string("  match result {\n")
      buf.write_string(
        "    Some(tree) => @agents.decode_output(tree, \"\{ctx}\")\n",
      )
      buf.write_string(
        "    None => raise @common.AgentError::InvalidType(\"\{ctx}: expected a return value, got none\")\n",
      )
      buf.write_string("  }\n")
    }
  }
  buf.write_string("}\n\n")
  // trigger
  buf.write_string("///|\n")
  buf.write_string(
    "pub fn \{client}::trigger_\{meth.name}(\n  \{self_args},\n) -> Unit raise @common.AgentError {\n",
  )
  buf.write_string("  let input = \{input_expr}\n")
  buf.write_string(
    "  self.client.invoke(\"\{escape_string_lit(meth.name)}\", input)\n}\n\n",
  )
  // schedule
  let sched_args = "self : \{client}, scheduled_at : @wallClock.Datetime" +
    (if arg_sig.is_empty() { "" } else { ", " + arg_sig })
  buf.write_string("///|\n")
  buf.write_string(
    "pub fn \{client}::schedule_\{meth.name}(\n  \{sched_args},\n) -> Unit raise @common.AgentError {\n",
  )
  buf.write_string("  let input = \{input_expr}\n")
  buf.write_string(
    "  self.client.schedule_invocation(scheduled_at, \"\{escape_string_lit(meth.name)}\", input)\n}\n\n",
  )
  // schedule_cancelable
  buf.write_string("///|\n")
  buf.write_string(
    "pub fn \{client}::schedule_cancelable_\{meth.name}(\n  \{sched_args},\n) -> @agentHost.CancellationToken raise @common.AgentError {\n",
  )
  buf.write_string("  let input = \{input_expr}\n")
  buf.write_string(
    "  self.client.schedule_cancelable_invocation(scheduled_at, \"\{escape_string_lit(meth.name)}\", input)\n}\n\n",
  )
}

///|
/// Emits the `*_with_config` client variants for agents with overridable config.
fn emit_client_config_variants(
  buf : StringBuilder,
  agent : AgentInfo,
  client : String,
  vis : Array[(String, ParamType)],
  override_name : String,
  walker : String,
  durable : Bool,
) -> Unit {
  let ctor_input = encode_input_expr(vis)
  let ctor_sig = params_sig(vis)
  let comma_sig = if ctor_sig.is_empty() { "" } else { ctor_sig + ",\n  " }
  // get_with_config (durable only)
  if durable {
    buf.write_string("///|\n")
    buf.write_string(
      "pub fn \{client}::get_with_config(\n  \{comma_sig}config : \{override_name},\n) -> \{client} raise @common.AgentError {\n",
    )
    buf.write_string("  let constructor_input = \{ctor_input}\n")
    buf.write_string("  let agent_config = \{walker}(config, [])\n")
    buf.write_string(
      "  let client = @rpc.AgentClient::get_with_config(\"\{escape_string_lit(agent.name)}\", constructor_input, agent_config)\n",
    )
    buf.write_string("  { client, }\n}\n\n")
  }
  // new_phantom_with_config
  buf.write_string("///|\n")
  buf.write_string(
    "pub fn \{client}::new_phantom_with_config(\n  \{comma_sig}config : \{override_name},\n) -> \{client} raise @common.AgentError {\n",
  )
  buf.write_string("  let constructor_input = \{ctor_input}\n")
  buf.write_string("  let agent_config = \{walker}(config, [])\n")
  buf.write_string(
    "  let client = @rpc.AgentClient::new_phantom_with_config(\"\{escape_string_lit(agent.name)}\", constructor_input, agent_config)\n",
  )
  buf.write_string("  { client, }\n}\n\n")
  // get_phantom_with_config
  buf.write_string("///|\n")
  buf.write_string(
    "pub fn \{client}::get_phantom_with_config(\n  \{comma_sig}phantom_id : @types.Uuid,\n  config : \{override_name},\n) -> \{client} raise @common.AgentError {\n",
  )
  buf.write_string("  let constructor_input = \{ctor_input}\n")
  buf.write_string("  let agent_config = \{walker}(config, [])\n")
  buf.write_string(
    "  let client = @rpc.AgentClient::get_phantom_with_config(\"\{escape_string_lit(agent.name)}\", constructor_input, phantom_id, agent_config)\n",
  )
  buf.write_string("  { client, }\n}\n\n")
  // scoped variants
  if durable {
    emit_client_scoped_with_config(
      buf,
      client,
      "scoped_with_config",
      "get_with_config",
      vis,
      [],
      override_name,
    )
  }
  emit_client_scoped_with_config(
    buf,
    client,
    "scoped_new_phantom_with_config",
    "new_phantom_with_config",
    vis,
    [],
    override_name,
  )
  emit_client_scoped_with_config(
    buf,
    client,
    "scoped_get_phantom_with_config",
    "get_phantom_with_config",
    vis,
    [("phantom_id", "@types.Uuid")],
    override_name,
  )
}

///|
/// Emits a `scoped*_with_config` convenience method.
fn emit_client_scoped_with_config(
  buf : StringBuilder,
  client : String,
  fn_name : String,
  ctor_name : String,
  vis : Array[(String, ParamType)],
  extra : Array[(String, String)],
  override_name : String,
) -> Unit {
  let sig_parts : Array[String] = []
  for p in vis {
    sig_parts.push("\{p.0} : \{param_type_to_type_string(p.1)}")
  }
  for e in extra {
    sig_parts.push("\{e.0} : \{e.1}")
  }
  sig_parts.push("config : \{override_name}")
  sig_parts.push("f : (\{client}) -> T raise @common.AgentError")
  let call_args : Array[String] = []
  for p in vis {
    call_args.push(p.0)
  }
  for e in extra {
    call_args.push(e.0)
  }
  call_args.push("config")
  let sig = sig_parts.join(",\n  ")
  let call = call_args.join(", ")
  buf.write_string("///|\n")
  buf.write_string(
    "pub fn[T] \{client}::\{fn_name}(\n  \{sig},\n) -> T raise @common.AgentError {\n",
  )
  buf.write_string("  let client = \{client}::\{ctor_name}(\{call})\n")
  buf.write_string("  defer client.drop()\n")
  buf.write_string("  f(client)\n}\n\n")
}

///|
/// Resolves the `ConfigTypeInfo` for an agent's `config_type` reference.
fn resolve_config_type(
  agent : AgentInfo,
  config_types : Array[ConfigTypeInfo],
  agent_package : String,
) -> ConfigTypeInfo? {
  match agent.config_type {
    None => None
    Some(type_name) =>
      if decode_qualified(type_name) is Some(_) {
        for ct in config_types {
          if ct.package.length() > 0 {
            let alias = last_path_segment(ct.package)
            if "@\{alias}.\{ct.name}" == type_name {
              return Some(ct)
            }
          }
        }
        None
      } else {
        for ct in config_types {
          if ct.name == type_name && ct.package == agent_package {
            return Some(ct)
          }
        }
        for ct in config_types {
          if ct.name == type_name {
            return Some(ct)
          }
        }
        None
      }
  }
}
