// Emission of typed RPC client stubs (`golem_clients.mbt`) for the recursive
// schema model (`golem:core/types@2.0.0`).
//
// Each agent gets a `<Name>Client` wrapping `@rpc.AgentClient`. Constructor and
// method arguments are encoded into a `@types.SchemaValueTree` input record.
// Awaited methods use `@agents.encode_invocation_input_async` and decode results
// via `@agents.decode_output`, allowing native streams at any nesting depth.
// Synchronous setup, trigger, and scheduling methods use
// `@agents.encode_invocation_input` and keep the SDK's `@common.AgentError`
// contract.
//
// Built as a MoonBit-source string and parsed back via `parse_generated_impls`
// (see `value_types_emit.mbt` / `agents_emit.mbt`).

///|
/// Checks whether a type contains an agent stream, resolving local value types
/// recursively. `visited` is path-local so cycles terminate without hiding a
/// stream reachable through a sibling field.
fn param_type_contains_agent_stream(
  pt : ParamType,
  value_types : Map[String, ValueTypeInfo],
  visited : Array[String],
) -> Bool {
  match pt {
    Simple(name) => {
      if visited.contains(name) {
        return false
      }
      match value_types.get(name) {
        Some(value_type) => {
          let next_visited = visited.copy()
          next_visited.push(name)
          value_type_contains_agent_stream(
            value_type, value_types, next_visited,
          )
        }
        None => false
      }
    }
    Optional(inner) | List(inner) | MultimodalType(inner) =>
      param_type_contains_agent_stream(inner, value_types, visited)
    ResultType(ok, err) =>
      param_type_contains_agent_stream(ok, value_types, visited) ||
      param_type_contains_agent_stream(err, value_types, visited)
    Tuple(elems) =>
      elems
      .iter()
      .any(fn(elem) {
        param_type_contains_agent_stream(elem, value_types, visited)
      })
    Parameterized(name, params) =>
      name == "AgentStream" ||
      name == "@schema.AgentStream" ||
      params
      .iter()
      .any(fn(param) {
        param_type_contains_agent_stream(param, value_types, visited)
      })
  }
}

///|
fn value_type_contains_agent_stream(
  value_type : ValueTypeInfo,
  value_types : Map[String, ValueTypeInfo],
  visited : Array[String],
) -> Bool {
  match value_type.kind {
    Record(fields) =>
      fields
      .iter()
      .any(fn(field) {
        param_type_contains_agent_stream(field.1, value_types, visited)
      })
    SimpleEnum(_) => false
    VariantEnum(cases) =>
      cases
      .iter()
      .any(fn(case) {
        match case.1 {
          VariantPayload::None => false
          Single(payload) =>
            param_type_contains_agent_stream(payload, value_types, visited)
          Multi(fields, _) =>
            fields
            .iter()
            .any(fn(field) {
              param_type_contains_agent_stream(field.1, value_types, visited)
            })
        }
      })
  }
}

///|
/// Returns the first client-visible constructor parameter containing a stream.
fn stream_bearing_constructor_param(
  agent : AgentInfo,
  value_types : Map[String, ValueTypeInfo],
) -> (String, ParamType)? {
  for param in visible_params(agent.constructor_params) {
    if param_type_contains_agent_stream(param.1, value_types, []) {
      return Some(param)
    }
  }
  None
}

///|
/// Checks if any client-visible parameter or return type contains a stream.
fn method_contains_agent_stream(
  meth : MethodInfo,
  value_types : Map[String, ValueTypeInfo],
) -> Bool {
  for param in visible_params(meth.params) {
    if param_type_contains_agent_stream(param.1, value_types, []) {
      return true
    }
  }
  match meth.return_type {
    Some(pt) => param_type_contains_agent_stream(pt, value_types, [])
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
/// Renders the `@agents.encode_wire_input([...])` expression that encodes
/// the user-supplied parameters (in declared order) into a wire input record.
fn encode_input_expr(params : Array[(String, ParamType)]) -> String {
  let parts = params.map(fn(p) { p.0 }).join(", ")
  "@agents.encode_wire_input([\{parts}])"
}

///|
/// Renders the async input encoder used only by awaited RPC methods.
fn encode_input_async_expr(params : Array[(String, ParamType)]) -> String {
  let parts = params.map(fn(p) { p.0 }).join(", ")
  "@agents.encode_wire_input_async([\{parts}])"
}

///|
/// The optional label of awaited client methods that attaches a scope card.
const SCOPE_CARD_LABEL : String = "scope_card"

///|
/// The generated client method that sets the default scope card. An agent
/// method with this name would produce a duplicate definition.
const SET_DEFAULT_SCOPE_CARD : String = "set_default_scope_card"

///|
/// The generated method of durable clients that returns the agent ID. An agent
/// method with this name would produce a duplicate definition.
const AGENT_ID_METHOD : String = "agent_id"

///|
/// The optional labels of the generated `create`. They share the scope of the
/// constructor parameters.
let create_labels : Array[String] = ["keyed", "config", "env"]

///|
/// Renames a positional parameter named like the `scope_card` label to a name
/// that no other parameter of the method uses. Positional parameters are not
/// part of the call syntax, so renaming them does not affect callers.
fn rename_scope_card_param(
  params : Array[(String, ParamType)],
) -> Array[(String, ParamType)] {
  rename_label_params(params, [SCOPE_CARD_LABEL])
}

///|
/// Renames every positional parameter named like one of `labels` to a name
/// that no other parameter and no label uses.
fn rename_label_params(
  params : Array[(String, ParamType)],
  labels : Array[String],
) -> Array[(String, ParamType)] {
  let taken : Array[String] = params.map(p => p.0)
  taken.append(labels)
  params.map(fn(p) {
    if !labels.contains(p.0) {
      return p
    }
    let mut candidate = p.0 + "_"
    while taken.contains(candidate) {
      candidate = candidate + "_"
    }
    taken.push(candidate)
    (candidate, p.1)
  })
}

///|
/// Generates typed RPC client stubs for all agents.
pub fn generate_client_code(
  agents : Array[AgentInfo],
  value_types~ : Array[ValueTypeInfo] = [],
  config_types~ : Array[ConfigTypeInfo] = [],
  agent_package~ : String = "",
) -> @list.List[@syntax.Impl] raise AgentParseError {
  let value_types_by_name : Map[String, ValueTypeInfo] = Map([])
  for value_type in value_types {
    value_types_by_name[value_type.name] = value_type
  }
  let buf = StringBuilder()
  for agent in agents {
    if agent.is_http_router() {
      continue
    }
    let helper_owners : Map[String, String] = Map([])
    for meth in agent.methods {
      if !method_contains_agent_stream(meth, value_types_by_name) {
        for
          helper in [
            "trigger_\{meth.name}",
            "schedule_\{meth.name}",
            "get_\{meth.name}_invocation",
          ] {
          helper_owners[helper] = meth.name
        }
      }
    }
    for meth in agent.methods {
      if helper_owners.get(meth.name) is Some(owner) {
        raise AgentParseError(
          "Cannot generate native client for agent '\{agent.name}': method name '\{meth.name}' is reserved for a generated invocation helper of method '\{owner}'",
        )
      }
      if meth.name == "bind_self" && agent.mode is Durable {
        raise AgentParseError(
          "Cannot generate native client for agent '\{agent.name}': method name 'bind_self' is reserved for the generated self-binding method",
        )
      }
      if meth.name == AGENT_ID_METHOD && agent.mode is Durable {
        raise AgentParseError(
          "Cannot generate native client for agent '\{agent.name}': method name '\{AGENT_ID_METHOD}' is reserved for the generated method that returns the agent ID",
        )
      }
      if meth.name == SET_DEFAULT_SCOPE_CARD {
        raise AgentParseError(
          "Cannot generate native client for agent '\{agent.name}': method name '\{SET_DEFAULT_SCOPE_CARD}' is reserved for the generated client method that sets the default scope card",
        )
      }
    }
    match stream_bearing_constructor_param(agent, value_types_by_name) {
      Some(param) =>
        raise AgentParseError(
          "Cannot generate native client for agent '\{agent.name}': constructor parameter '\{param.0}' contains AgentStream, but native client construction is synchronous; move the stream to an agent method parameter or return type",
        )
      None => ()
    }
    emit_client(buf, agent, value_types_by_name, config_types, agent_package)
  }
  let source = buf.to_string()
  if source.is_empty() {
    return @list.List([])
  }
  parse_generated_impls(source, "clients")
}

///|
/// The config override carrier of a durable agent's generated `create`: the
/// generated `*Override` type and the walker that collects its leaf values.
priv struct ConfigOverride {
  type_name : String
  walker : String
}

///|
/// Emits the full client surface for a single agent.
fn emit_client(
  buf : StringBuilder,
  agent : AgentInfo,
  value_types : Map[String, ValueTypeInfo],
  config_types : Array[ConfigTypeInfo],
  agent_package : String,
) -> Unit {
  let client = agent.name + "Client"
  let type_name = escape_string_lit(agent.name)
  let vis = visible_params(agent.constructor_params)
  let ctor_sig = params_sig(vis)
  let ctor_input = encode_input_expr(vis)
  let durable = match agent.mode {
    Durable => true
    Ephemeral => false
  }
  // struct
  buf.write_string("///|\n")
  buf.write_string(
    "pub(all) struct \{client} {\n  client : @rpc.AgentClient\n}\n\n",
  )
  if durable {
    buf.write_string("///|\n")
    buf.write_string(
      "/// Binds the current durable incarnation without creating it or waiting for initialization.\n///\n/// Use this to schedule work on self from inside the constructor. `upsert` reuses\n/// an initialized agent without rerunning its constructor, but waits for an unfinished\n/// constructor. Calling `upsert` on self from that constructor would therefore deadlock.\n///\n/// Scheduled work runs after construction finishes. This does not make awaited\n/// self-calls reentrant or bypass method permissions. A different client type raises\n/// an error. Replay preserves the recorded incarnation.\n",
    )
    buf.write_string(
      "pub fn \{client}::bind_self() -> \{client} raise {\n  { client: @rpc.AgentClient::bind_self(\"\{type_name}\"), }\n}\n\n",
    )
  }
  // upsert
  buf.write_string("///|\n")
  if durable {
    buf.write_string(
      "/// Connects to the agent that the constructor parameters key. The host creates\n/// the agent when it is missing and reuses it otherwise.\n",
    )
  } else {
    buf.write_string(
      "/// Addresses the ephemeral agent type. Every invocation runs on a new one-shot\n/// instance, whose ID the invocation metadata returns.\n",
    )
  }
  buf.write_string(
    "pub fn \{client}::upsert(\{ctor_sig}) -> \{client} raise {\n",
  )
  buf.write_string("  let constructor_input = \{ctor_input}\n")
  buf.write_string(
    "  let client = @rpc.AgentClient::upsert(\"\{type_name}\", constructor_input)\n",
  )
  buf.write_string("  { client, }\n}\n\n")
  let config = if durable {
    resolve_config_override(agent, config_types, agent_package)
  } else {
    None
  }
  if durable {
    // get_by_id
    buf.write_string("///|\n")
    buf.write_string(
      "/// Connects to the existing agent with the given ID. Never creates an agent: an\n/// unknown ID, or the ID of an agent of another type, raises `RpcError::NotFound`.\n",
    )
    buf.write_string(
      "pub fn \{client}::get_by_id(agent_id : @schema.AgentId) -> \{client} raise @agentHost.RpcError {\n",
    )
    buf.write_string(
      "  let client = @rpc.AgentClient::get_by_id(\"\{type_name}\", agent_id)\n",
    )
    buf.write_string("  { client, }\n}\n\n")
    emit_client_create(buf, client, type_name, vis, config)
    // agent_id
    buf.write_string("///|\n")
    buf.write_string(
      "/// The ID of the agent. On a client of `upsert`, the host creates the agent when\n/// it is missing, without invoking a method.\n",
    )
    buf.write_string(
      "pub async fn \{client}::\{AGENT_ID_METHOD}(self : \{client}) -> @schema.AgentId {\n  self.client.agent_id()\n}\n\n",
    )
  }
  // drop
  buf.write_string("///|\n")
  buf.write_string(
    "pub fn \{client}::drop(self : \{client}) -> Unit {\n  self.client.drop()\n}\n\n",
  )
  // default scope card for awaited invocations
  buf.write_string("///|\n")
  buf.write_string(
    "pub fn \{client}::\{SET_DEFAULT_SCOPE_CARD}(self : \{client}, card : @permissions.ScopeCard?) -> Unit {\n  self.client.set_default_scope_card(card)\n}\n\n",
  )
  emit_client_scoped(
    buf,
    client,
    "scoped_upsert",
    vis.map(p => "\{p.0} : \{param_type_to_type_string(p.1)}"),
    "\{client}::upsert(\{vis.map(p => p.0).join(", ")})",
  )
  if durable {
    emit_client_scoped(
      buf,
      client,
      "scoped_get_by_id",
      ["agent_id : @schema.AgentId"],
      "\{client}::get_by_id(agent_id)",
    )
    let create_params = rename_label_params(vis, create_labels)
    let sig = create_params.map(p => {
      "\{p.0} : \{param_type_to_type_string(p.1)}"
    })
    sig.append(create_option_params(config))
    let args = create_params.map(p => p.0)
    args.push("keyed~")
    if config is Some(_) {
      args.push("config?")
    }
    args.push("env~")
    emit_client_scoped(
      buf,
      client,
      "scoped_create",
      sig,
      "\{client}::create(\{args.join(", ")})",
    )
  }
  // methods
  for meth in agent.methods {
    let contains_stream = method_contains_agent_stream(meth, value_types)
    emit_client_method(buf, agent, meth, client, durable, contains_stream)
  }
}

///|
/// Resolves the config override carrier of an agent. It exists only when the
/// config emitter generated an `*Override` type and walker for the agent's
/// config type, which the same recursive "has overridable leaves" predicate
/// decides.
fn resolve_config_override(
  agent : AgentInfo,
  config_types : Array[ConfigTypeInfo],
  agent_package : String,
) -> ConfigOverride? {
  guard resolve_config_type(agent, config_types, agent_package) is Some(ct) else {
    return None
  }
  let by_name : Map[String, ConfigTypeInfo] = Map([])
  for c in config_types {
    by_name[c.name] = c
  }
  if !config_has_overridable_fields(ct, by_name) {
    return None
  }
  Some({
    type_name: ct.name + "Override",
    walker: "collect_" + pascal_to_snake(ct.name) + "_overrides",
  })
}

///|
/// The optional parameters of the generated `create`.
fn create_option_params(config : ConfigOverride?) -> Array[String] {
  let params = ["keyed? : Bool = false"]
  if config is Some(config) {
    params.push("config? : \{config.type_name}")
  }
  params.push("env? : Array[(String, String)] = []")
  params
}

///|
/// Emits the `create` of a durable client.
fn emit_client_create(
  buf : StringBuilder,
  client : String,
  type_name : String,
  vis : Array[(String, ParamType)],
  config : ConfigOverride?,
) -> Unit {
  let params = rename_label_params(vis, create_labels)
  let sig = params.map(p => "\{p.0} : \{param_type_to_type_string(p.1)}")
  sig.append(create_option_params(config))
  buf.write_string("///|\n")
  buf.write_string(
    "/// Creates a new agent and returns after its initialization. With `keyed`, the new\n/// agent is the agent that the constructor parameters key, and the creation raises\n/// `RpcError::Conflict` when that agent already exists; otherwise the new agent has\n/// no key and a new ID. `env` adds environment variables to the ones the manifest\n/// defines.\n",
  )
  buf.write_string(
    "pub async fn \{client}::create(\n  \{sig.join(",\n  ")},\n) -> \{client} {\n",
  )
  buf.write_string("  let constructor_input = \{encode_input_expr(params)}\n")
  match config {
    Some(config) => {
      buf.write_string("  let agent_config = match config {\n")
      buf.write_string("    Some(config) => \{config.walker}(config, [])\n")
      buf.write_string("    None => []\n  }\n")
      buf.write_string(
        "  let client = @rpc.AgentClient::create(\"\{type_name}\", constructor_input, keyed~, config=agent_config, env~)\n",
      )
    }
    None =>
      buf.write_string(
        "  let client = @rpc.AgentClient::create(\"\{type_name}\", constructor_input, keyed~, env~)\n",
      )
  }
  buf.write_string("  { client, }\n}\n\n")
}

///|
/// Emits a `scoped*` convenience method that acquires a client with `acquire`,
/// defers `drop`, and runs the callback.
fn emit_client_scoped(
  buf : StringBuilder,
  client : String,
  fn_name : String,
  params : Array[String],
  acquire : String,
) -> Unit {
  let sig_parts = params.copy()
  sig_parts.push("f : async (\{client}) -> T")
  let sig = sig_parts.join(",\n  ")
  buf.write_string("///|\n")
  buf.write_string(
    "pub async fn[T] \{client}::\{fn_name}(\n  \{sig},\n) -> T {\n",
  )
  buf.write_string("  let client = \{acquire}\n")
  buf.write_string("  defer client.drop()\n")
  buf.write_string("  f(client)\n}\n\n")
}

///|
/// Emits the awaited client method and, for stream-free signatures, the async
/// trigger and schedule variants, which return an invocation handle, and the
/// `get_*_invocation` that rebuilds a handle from its ID.
fn emit_client_method(
  buf : StringBuilder,
  agent : AgentInfo,
  meth : MethodInfo,
  client : String,
  durable : Bool,
  contains_stream : Bool,
) -> Unit {
  // The awaited method takes a `scope_card?` label.
  let vis = rename_scope_card_param(visible_params(meth.params))
  let ctx = escape_string_lit("\{agent.name}.\{meth.name}")
  let input_expr = encode_input_expr(vis)
  let awaited_input_expr = encode_input_async_expr(vis)
  let arg_sig = params_sig(vis)
  let self_args = if arg_sig.is_empty() {
    "self : \{client}"
  } else {
    "self : \{client}, \{arg_sig}"
  }
  let value_ret = match meth.return_type {
    None => "Unit"
    Some(pt) => param_type_to_type_string(pt)
  }
  let ret = if durable {
    value_ret
  } else {
    "@rpc.InvocationResult[\{value_ret}]"
  }
  // awaited invoke
  buf.write_string("///|\n")
  buf.write_string(
    "pub async fn \{client}::\{meth.name}(\n  \{self_args},\n  \{SCOPE_CARD_LABEL}? : @permissions.ScopeCard,\n) -> \{ret} {\n",
  )
  // A dropped scope card aborts before the input encoding moves any argument
  // card.
  buf.write_string("  self.client.check_scope_card(\{SCOPE_CARD_LABEL}?)\n")
  buf.write_string("  let input = \{awaited_input_expr}\n")
  buf.write_string(
    "  let response = self.client.invoke_and_await(\"\{escape_string_lit(meth.name)}\", input, \{SCOPE_CARD_LABEL}?)\n",
  )
  match meth.return_type {
    None =>
      buf.write_string(
        if durable {
          "  @agents.discard_optional_wire_value(response.value)\n"
        } else {
          "  @agents.discard_optional_wire_value(response.value)\n  { metadata: response.metadata, value: () }\n"
        },
      )
    Some(_) => {
      buf.write_string("  let result = response.value\n")
      buf.write_string("  match result {\n")
      buf.write_string(
        if durable {
          "    Some(tree) => @agents.decode_wire_output(tree, \"\{ctx}\")\n"
        } else {
          "    Some(tree) => { metadata: response.metadata, value: @agents.decode_wire_output(tree, \"\{ctx}\") }\n"
        },
      )
      buf.write_string(
        "    None => raise @common.AgentError::InvalidType(\"\{ctx}: expected a return value, got none\")\n",
      )
      buf.write_string("  }\n")
    }
  }
  buf.write_string("}\n\n")
  if contains_stream {
    return
  }
  let handle = "@rpc.Invocation[\{value_ret}]"
  let decode = match meth.return_type {
    None => "output => @agents.discard_optional_wire_value(output)"
    Some(_) =>
      "output => match output {\n    Some(tree) => @agents.decode_wire_output(tree, \"\{ctx}\")\n    None => raise @common.AgentError::InvalidType(\"\{ctx}: expected a return value, got none\")\n  }"
  }
  let method_name_lit = escape_string_lit(meth.name)
  // trigger
  buf.write_string("///|\n")
  buf.write_string(
    "/// Starts `\{meth.name}` without waiting for it to run, and returns the handle of\n/// the invocation once the target has accepted it.\n",
  )
  buf.write_string(
    "pub async fn \{client}::trigger_\{meth.name}(\n  \{self_args},\n) -> \{handle} {\n",
  )
  buf.write_string("  let input = \{input_expr}\n")
  buf.write_string(
    "  self.client.trigger(\"\{method_name_lit}\", input, \{decode})\n}\n\n",
  )
  // schedule
  let sched_args = "self : \{client}, scheduled_at : @systemClock.Instant" +
    (if arg_sig.is_empty() { "" } else { ", " + arg_sig })
  buf.write_string("///|\n")
  buf.write_string(
    "/// Schedules `\{meth.name}` for `scheduled_at`, and returns the handle of the\n/// invocation, which can cancel it until it starts.\n",
  )
  buf.write_string(
    "pub async fn \{client}::schedule_\{meth.name}(\n  \{sched_args},\n) -> \{handle} {\n",
  )
  buf.write_string("  let input = \{input_expr}\n")
  buf.write_string(
    "  self.client.schedule(scheduled_at, \"\{method_name_lit}\", input, \{decode})\n}\n\n",
  )
  // rebuild a handle from its ID
  buf.write_string("///|\n")
  buf.write_string(
    "/// Rebuilds the handle of an invocation of `\{meth.name}` from the ID that\n/// `trigger_\{meth.name}` or `schedule_\{meth.name}` returned. Makes no remote call.\n",
  )
  buf.write_string(
    "pub fn \{client}::get_\{meth.name}_invocation(id : @rpc.InvocationId) -> \{handle} {\n",
  )
  buf.write_string("  @rpc.Invocation::get(id, \{decode})\n}\n\n")
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
          if ct.package_path.length() > 0 {
            let package_alias = last_path_segment(ct.package_path)
            if "@\{package_alias}.\{ct.name}" == type_name {
              return Some(ct)
            }
          }
        }
        None
      } else {
        for ct in config_types {
          if ct.name == type_name && ct.package_path == agent_package {
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
