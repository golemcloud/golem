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
/// Generates typed RPC client stubs for all agents. Each agent gets a client
/// struct with invoke, trigger, and schedule methods.
pub fn generate_client_code(
  agents : Array[AgentInfo],
  config_types? : Array[ConfigTypeInfo] = [],
  agent_package? : String = "",
) -> @list.List[@syntax.Impl] {
  let impls : Array[@syntax.Impl] = []
  for agent in agents {
    // Skip agents with RPC-incompatible constructors
    if has_rpc_incompatible_constructor(agent) {
      continue
    }
    let client_name = agent.name + "Client"
    impls.push(build_client_struct(client_name))
    match agent.mode {
      Durable => impls.push(build_client_get(agent, client_name))
      Ephemeral => ()
    }
    impls.push(build_client_new_phantom(agent, client_name))
    impls.push(build_client_get_phantom(agent, client_name))
    impls.push(build_client_get_agent_id(client_name))
    impls.push(build_client_phantom_id(client_name))
    impls.push(build_client_drop(client_name))
    match agent.mode {
      Durable => impls.push(build_client_scoped(agent, client_name))
      Ephemeral => ()
    }
    impls.push(build_client_scoped_new_phantom(agent, client_name))
    impls.push(build_client_scoped_get_phantom(agent, client_name))
    for meth in agent.methods {
      // Skip methods with RPC-incompatible params/return types
      if method_has_rpc_incompatible_types(meth) {
        continue
      }
      impls.push(build_client_method_invoke(agent, meth, client_name))
      impls.push(build_client_method_trigger(agent, meth, client_name))
      impls.push(build_client_method_schedule(agent, meth, client_name))
    }
    // Generate _with_config variants for agents with non-secret config fields
    let resolved_ct = resolve_config_type(agent, config_types, agent_package)
    match resolved_ct {
      Some(ct) =>
        if has_non_secret_config_fields(ct) {
          let override_name = ct.name + "Override"
          let walker_name = "collect_" + pascal_to_snake(ct.name) + "_overrides"
          match agent.mode {
            Durable =>
              impls.push(
                build_client_get_with_config(
                  agent, client_name, override_name, walker_name,
                ),
              )
            Ephemeral => ()
          }
          impls.push(
            build_client_new_phantom_with_config(
              agent, client_name, override_name, walker_name,
            ),
          )
          impls.push(
            build_client_get_phantom_with_config(
              agent, client_name, override_name, walker_name,
            ),
          )
          match agent.mode {
            Durable =>
              impls.push(
                build_client_scoped_with_config(agent, client_name, override_name),
              )
            Ephemeral => ()
          }
          impls.push(
            build_client_scoped_new_phantom_with_config(
              agent, client_name, override_name,
            ),
          )
          impls.push(
            build_client_scoped_get_phantom_with_config(
              agent, client_name, override_name,
            ),
          )
        }
      None => ()
    }
  }
  to_list(impls)
}

///|
/// Generates the client struct definition containing an `AgentClient` field.
fn build_client_struct(client_name : String) -> @syntax.Impl {
  make_struct_def(client_name, [
    ("client", make_qualified_parameterized_type("rpc", "AgentClient", [])),
  ])
}

///|
/// Generates the expression that serializes constructor parameters into a `DataValue`.
/// Principal parameters are excluded — they are auto-injected by the runtime.
fn build_constructor_data_expr(agent : AgentInfo) -> @syntax.Expr {
  let vis = visible_params(agent.constructor_params)
  // Check if the constructor has a multimodal parameter (must be the only visible param)
  let has_multimodal = vis.iter().find_first(fn(p) { is_multimodal_type(p.1) })
  if has_multimodal is Some(_) {
    // Multimodal constructor: param.to_data_value()
    let param_name = vis[0].0
    make_dot_apply(make_ident_expr(param_name), "to_data_value", [])
  } else {
    let elements = vis.map(fn(param) {
      make_apply(make_qualified_expr("schema", "to_element_value_as"), [
        make_ident_expr(param.0),
      ])
    })
    make_apply(make_qualified_constr("rpcTypes", "DataValue", "Tuple"), [
      make_array(elements),
    ])
  }
}

///|
/// Generates the `get` constructor that creates a client by initializing the remote agent.
fn build_client_get(agent : AgentInfo, client_name : String) -> @syntax.Impl {
  let vis = visible_params(agent.constructor_params)
  let params : Array[@syntax.Parameter] = vis.map(fn(p) {
    make_typed_param(p.0, param_type_to_ast_type(p.1))
  })
  let body = make_let(
    "constructor_data",
    build_constructor_data_expr(agent),
    make_let(
      "client",
      make_apply(make_qualified_method_ref("rpc", "AgentClient", "get"), [
        make_string_expr(agent.name),
        make_ident_expr("constructor_data"),
      ]),
      make_record([make_field("client", make_ident_expr("client"))]),
    ),
  )
  make_top_func(
    Some(client_name),
    "get",
    params,
    Some(make_type(client_name)),
    true,
    body,
  )
}

///|
/// Generates the `new_phantom` constructor that creates a phantom client with an auto-generated UUID.
fn build_client_new_phantom(
  agent : AgentInfo,
  client_name : String,
) -> @syntax.Impl {
  let vis = visible_params(agent.constructor_params)
  let params : Array[@syntax.Parameter] = vis.map(fn(p) {
    make_typed_param(p.0, param_type_to_ast_type(p.1))
  })
  let body = make_let(
    "constructor_data",
    build_constructor_data_expr(agent),
    make_let(
      "client",
      make_apply(
        make_qualified_method_ref("rpc", "AgentClient", "new_phantom"),
        [make_string_expr(agent.name), make_ident_expr("constructor_data")],
      ),
      make_record([make_field("client", make_ident_expr("client"))]),
    ),
  )
  make_top_func(
    Some(client_name),
    "new_phantom",
    params,
    Some(make_type(client_name)),
    true,
    body,
  )
}

///|
/// Generates the `get_phantom` constructor that creates a phantom client with a caller-supplied UUID.
fn build_client_get_phantom(
  agent : AgentInfo,
  client_name : String,
) -> @syntax.Impl {
  let vis = visible_params(agent.constructor_params)
  let params : Array[@syntax.Parameter] = vis.map(fn(p) {
    make_typed_param(p.0, param_type_to_ast_type(p.1))
  })
  params.push(
    make_typed_param(
      "phantom_id",
      make_qualified_parameterized_type("rpcTypes", "Uuid", []),
    ),
  )
  let body = make_let(
    "constructor_data",
    build_constructor_data_expr(agent),
    make_let(
      "client",
      make_apply(
        make_qualified_method_ref("rpc", "AgentClient", "get_phantom"),
        [
          make_string_expr(agent.name),
          make_ident_expr("constructor_data"),
          make_ident_expr("phantom_id"),
        ],
      ),
      make_record([make_field("client", make_ident_expr("client"))]),
    ),
  )
  make_top_func(
    Some(client_name),
    "get_phantom",
    params,
    Some(make_type(client_name)),
    true,
    body,
  )
}

///|
/// Generates the `get_agent_id` method that returns the remote agent's ID.
fn build_client_get_agent_id(client_name : String) -> @syntax.Impl {
  let body = make_dot_apply(
    make_field_access(make_ident_expr("self"), "client"),
    "get_agent_id",
    [],
  )
  make_top_func(
    Some(client_name),
    "get_agent_id",
    [make_self_param(client_name)],
    Some(make_type("String")),
    true,
    body,
  )
}

///|
/// Generates the `phantom_id` method that returns the phantom UUID, if any.
fn build_client_phantom_id(client_name : String) -> @syntax.Impl {
  let body = make_dot_apply(
    make_field_access(make_ident_expr("self"), "client"),
    "phantom_id",
    [],
  )
  make_top_func(
    Some(client_name),
    "phantom_id",
    [make_self_param(client_name)],
    Some(
      make_option_type(
        make_qualified_parameterized_type("rpcTypes", "Uuid", []),
      ),
    ),
    false,
    body,
  )
}

///|
/// Generates the `drop` method that releases the remote agent handle.
fn build_client_drop(client_name : String) -> @syntax.Impl {
  let body = make_dot_apply(
    make_field_access(make_ident_expr("self"), "client"),
    "drop",
    [],
  )
  make_top_func(
    Some(client_name),
    "drop",
    [make_self_param(client_name)],
    Some(make_type("Unit")),
    false,
    body,
  )
}

///|
/// Builds a function type: (ClientType) -> T raise @common.AgentError
fn make_callback_type(client_name : String) -> @syntax.Type {
  @syntax.Type::Arrow(
    args=to_list([make_type(client_name)]),
    res=make_type("T"),
    err=@syntax.ErrorType::ErrorType(
      ty=make_qualified_parameterized_type("common", "AgentError", []),
    ),
    is_async=None,
    loc=dummy_loc,
  )
}

///|
/// Generates the `scoped` convenience method that acquires a client via `get`,
/// defers `drop`, and calls a callback.
fn build_client_scoped(agent : AgentInfo, client_name : String) -> @syntax.Impl {
  let vis = visible_params(agent.constructor_params)
  let params : Array[@syntax.Parameter] = vis.map(fn(p) {
    make_typed_param(p.0, param_type_to_ast_type(p.1))
  })
  params.push(make_typed_param("f", make_callback_type(client_name)))
  // Build body: let client = ClientName::get(args...); defer client.drop(); f(client)
  let get_args : Array[@syntax.Expr] = vis.map(fn(p) { make_ident_expr(p.0) })
  let body = make_let(
    "client",
    make_apply(make_method_ref(client_name, "get"), get_args),
    make_defer(
      make_dot_apply(make_ident_expr("client"), "drop", []),
      make_apply(make_ident_expr("f"), [make_ident_expr("client")]),
    ),
  )
  make_top_func_generic(
    Some(client_name),
    "scoped",
    ["T"],
    params,
    Some(make_type("T")),
    true,
    body,
  )
}

///|
/// Generates the `scoped_new_phantom` convenience method that acquires a client via
/// `new_phantom` (auto-generated UUID), defers `drop`, and calls a callback.
fn build_client_scoped_new_phantom(
  agent : AgentInfo,
  client_name : String,
) -> @syntax.Impl {
  let vis = visible_params(agent.constructor_params)
  let params : Array[@syntax.Parameter] = vis.map(fn(p) {
    make_typed_param(p.0, param_type_to_ast_type(p.1))
  })
  params.push(make_typed_param("f", make_callback_type(client_name)))
  let new_phantom_args : Array[@syntax.Expr] = vis.map(fn(p) {
    make_ident_expr(p.0)
  })
  let body = make_let(
    "client",
    make_apply(make_method_ref(client_name, "new_phantom"), new_phantom_args),
    make_defer(
      make_dot_apply(make_ident_expr("client"), "drop", []),
      make_apply(make_ident_expr("f"), [make_ident_expr("client")]),
    ),
  )
  make_top_func_generic(
    Some(client_name),
    "scoped_new_phantom",
    ["T"],
    params,
    Some(make_type("T")),
    true,
    body,
  )
}

///|
/// Generates the `scoped_get_phantom` convenience method that acquires a client via
/// `get_phantom` (caller-supplied UUID), defers `drop`, and calls a callback.
fn build_client_scoped_get_phantom(
  agent : AgentInfo,
  client_name : String,
) -> @syntax.Impl {
  let vis = visible_params(agent.constructor_params)
  let params : Array[@syntax.Parameter] = vis.map(fn(p) {
    make_typed_param(p.0, param_type_to_ast_type(p.1))
  })
  params.push(
    make_typed_param(
      "phantom_id",
      make_qualified_parameterized_type("rpcTypes", "Uuid", []),
    ),
  )
  params.push(make_typed_param("f", make_callback_type(client_name)))
  let get_phantom_args : Array[@syntax.Expr] = vis.map(fn(p) {
    make_ident_expr(p.0)
  })
  get_phantom_args.push(make_ident_expr("phantom_id"))
  let body = make_let(
    "client",
    make_apply(make_method_ref(client_name, "get_phantom"), get_phantom_args),
    make_defer(
      make_dot_apply(make_ident_expr("client"), "drop", []),
      make_apply(make_ident_expr("f"), [make_ident_expr("client")]),
    ),
  )
  make_top_func_generic(
    Some(client_name),
    "scoped_get_phantom",
    ["T"],
    params,
    Some(make_type("T")),
    true,
    body,
  )
}

///|
/// Generates the expression that serializes a single method parameter to an `ElementValue` for RPC.
fn build_rpc_param_expr(param : (String, ParamType)) -> @syntax.Expr {
  let (name, _pt) = param
  make_apply(make_qualified_expr("schema", "to_element_value_as"), [
    make_ident_expr(name),
  ])
}

///|
/// Generates the `DataValue` expression containing all serialized method parameters for RPC.
/// Principal parameters are excluded — they are auto-injected by the runtime.
fn build_rpc_params_expr(meth : MethodInfo) -> @syntax.Expr {
  let vis = visible_params(meth.params)
  // Check if the method has a single multimodal parameter
  let has_multimodal = vis.length() == 1 && is_multimodal_type(vis[0].1)
  if has_multimodal {
    let param_name = vis[0].0
    make_dot_apply(make_ident_expr(param_name), "to_data_value", [])
  } else {
    make_apply(make_qualified_constr("rpcTypes", "DataValue", "Tuple"), [
      make_array(vis.map(build_rpc_param_expr)),
    ])
  }
}

///|
/// Generates an invoke-and-await method on the client that calls the remote agent and returns the result.
fn build_client_method_invoke(
  agent : AgentInfo,
  meth : MethodInfo,
  client_name : String,
) -> @syntax.Impl {
  let rpc_name = rpc_function_name(agent.name, meth.name)
  let has_return = meth.return_type is Some(_)
  let vis = visible_params(meth.params)
  let has_multimodal_input = vis.length() == 1 && is_multimodal_type(vis[0].1)
  let params : Array[@syntax.Parameter] = [make_self_param(client_name)]
  for p in vis {
    params.push(make_typed_param(p.0, param_type_to_ast_type(p.1)))
  }
  let rpc_params = build_rpc_params_expr(meth)
  let invoke_expr = make_dot_apply(
    make_field_access(make_ident_expr("self"), "client"),
    "invoke_and_await",
    [make_string_expr(rpc_name), rpc_params],
  )
  let body = if has_return {
    let is_multimodal_output = match meth.return_type {
      Some(rt) => is_multimodal_type(rt)
      None => false
    }
    let decode_expr = if is_multimodal_output {
      make_apply(
        make_qualified_method_ref("types", "Multimodal", "from_data_value"),
        [make_ident_expr("result")],
      )
    } else {
      make_let(
        "elements",
        make_apply(make_qualified_expr("extractor", "extract_tuple"), [
          make_ident_expr("result"),
        ]),
        make_apply(make_qualified_expr("schema", "from_element_value_as"), [
          make_array_get(make_ident_expr("elements"), make_int_expr(0)),
        ]),
      )
    }
    make_let("result", invoke_expr, decode_expr)
  } else {
    make_let("_", invoke_expr, make_tuple([]))
  }
  let return_type = match meth.return_type {
    Some(pt) => Some(param_type_to_ast_type(pt))
    None => Some(make_type("Unit"))
  }
  make_top_func(
    Some(client_name),
    meth.name,
    params,
    return_type,
    has_return || has_multimodal_input || vis.length() > 0,
    body,
  )
}

///|
/// Generates a fire-and-forget trigger method on the client that invokes without waiting for a result.
fn build_client_method_trigger(
  agent : AgentInfo,
  meth : MethodInfo,
  client_name : String,
) -> @syntax.Impl {
  let rpc_name = rpc_function_name(agent.name, meth.name)
  let vis = visible_params(meth.params)
  let has_params = vis.length() > 0
  let params : Array[@syntax.Parameter] = [make_self_param(client_name)]
  for p in vis {
    params.push(make_typed_param(p.0, param_type_to_ast_type(p.1)))
  }
  let rpc_params = build_rpc_params_expr(meth)
  let body = make_dot_apply(
    make_field_access(make_ident_expr("self"), "client"),
    "invoke",
    [make_string_expr(rpc_name), rpc_params],
  )
  make_top_func(
    Some(client_name),
    "trigger_" + meth.name,
    params,
    Some(make_type("Unit")),
    has_params,
    body,
  )
}

///|
/// Generates a scheduled invocation method on the client that triggers at a specified time.
fn build_client_method_schedule(
  agent : AgentInfo,
  meth : MethodInfo,
  client_name : String,
) -> @syntax.Impl {
  let rpc_name = rpc_function_name(agent.name, meth.name)
  let vis = visible_params(meth.params)
  let has_params = vis.length() > 0
  let params : Array[@syntax.Parameter] = [make_self_param(client_name)]
  params.push(
    make_typed_param(
      "scheduled_at",
      make_qualified_parameterized_type("wallClock", "Datetime", []),
    ),
  )
  for p in vis {
    params.push(make_typed_param(p.0, param_type_to_ast_type(p.1)))
  }
  let rpc_params = build_rpc_params_expr(meth)
  let body = make_dot_apply(
    make_field_access(make_ident_expr("self"), "client"),
    "schedule_invocation",
    [make_ident_expr("scheduled_at"), make_string_expr(rpc_name), rpc_params],
  )
  make_top_func(
    Some(client_name),
    "schedule_" + meth.name,
    params,
    Some(make_type("Unit")),
    has_params,
    body,
  )
}

///|
/// Resolves the ConfigTypeInfo for an agent's config_type reference.
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

///|
fn has_non_secret_config_fields(ct : ConfigTypeInfo) -> Bool {
  ct.fields.iter().any(fn(f) { not(is_secret_type(f.1)) })
}

///|
fn build_config_walker_expr(walker_name : String) -> @syntax.Expr {
  make_apply_with_exclamation(make_ident_expr(walker_name), [
    make_ident_expr("config"),
    make_array([]),
  ])
}

///|
fn build_client_get_with_config(
  agent : AgentInfo,
  client_name : String,
  override_name : String,
  walker_name : String,
) -> @syntax.Impl {
  let vis = visible_params(agent.constructor_params)
  let params : Array[@syntax.Parameter] = vis.map(fn(p) {
    make_typed_param(p.0, param_type_to_ast_type(p.1))
  })
  params.push(make_typed_param("config", make_type(override_name)))
  let body = make_let(
    "constructor_data",
    build_constructor_data_expr(agent),
    make_let(
      "agent_config",
      build_config_walker_expr(walker_name),
      make_let(
        "client",
        make_apply(
          make_qualified_method_ref("rpc", "AgentClient", "get_with_config"),
          [
            make_string_expr(agent.name),
            make_ident_expr("constructor_data"),
            make_ident_expr("agent_config"),
          ],
        ),
        make_record([make_field("client", make_ident_expr("client"))]),
      ),
    ),
  )
  make_top_func(
    Some(client_name),
    "get_with_config",
    params,
    Some(make_type(client_name)),
    true,
    body,
  )
}

///|
fn build_client_new_phantom_with_config(
  agent : AgentInfo,
  client_name : String,
  override_name : String,
  walker_name : String,
) -> @syntax.Impl {
  let vis = visible_params(agent.constructor_params)
  let params : Array[@syntax.Parameter] = vis.map(fn(p) {
    make_typed_param(p.0, param_type_to_ast_type(p.1))
  })
  params.push(make_typed_param("config", make_type(override_name)))
  let body = make_let(
    "constructor_data",
    build_constructor_data_expr(agent),
    make_let(
      "agent_config",
      build_config_walker_expr(walker_name),
      make_let(
        "client",
        make_apply(
          make_qualified_method_ref(
            "rpc", "AgentClient", "new_phantom_with_config",
          ),
          [
            make_string_expr(agent.name),
            make_ident_expr("constructor_data"),
            make_ident_expr("agent_config"),
          ],
        ),
        make_record([make_field("client", make_ident_expr("client"))]),
      ),
    ),
  )
  make_top_func(
    Some(client_name),
    "new_phantom_with_config",
    params,
    Some(make_type(client_name)),
    true,
    body,
  )
}

///|
fn build_client_get_phantom_with_config(
  agent : AgentInfo,
  client_name : String,
  override_name : String,
  walker_name : String,
) -> @syntax.Impl {
  let vis = visible_params(agent.constructor_params)
  let params : Array[@syntax.Parameter] = vis.map(fn(p) {
    make_typed_param(p.0, param_type_to_ast_type(p.1))
  })
  params.push(
    make_typed_param(
      "phantom_id",
      make_qualified_parameterized_type("rpcTypes", "Uuid", []),
    ),
  )
  params.push(make_typed_param("config", make_type(override_name)))
  let body = make_let(
    "constructor_data",
    build_constructor_data_expr(agent),
    make_let(
      "agent_config",
      build_config_walker_expr(walker_name),
      make_let(
        "client",
        make_apply(
          make_qualified_method_ref(
            "rpc", "AgentClient", "get_phantom_with_config",
          ),
          [
            make_string_expr(agent.name),
            make_ident_expr("constructor_data"),
            make_ident_expr("phantom_id"),
            make_ident_expr("agent_config"),
          ],
        ),
        make_record([make_field("client", make_ident_expr("client"))]),
      ),
    ),
  )
  make_top_func(
    Some(client_name),
    "get_phantom_with_config",
    params,
    Some(make_type(client_name)),
    true,
    body,
  )
}

///|
fn build_client_scoped_with_config(
  agent : AgentInfo,
  client_name : String,
  override_name : String,
) -> @syntax.Impl {
  let vis = visible_params(agent.constructor_params)
  let params : Array[@syntax.Parameter] = vis.map(fn(p) {
    make_typed_param(p.0, param_type_to_ast_type(p.1))
  })
  params.push(make_typed_param("config", make_type(override_name)))
  params.push(make_typed_param("f", make_callback_type(client_name)))
  let get_args : Array[@syntax.Expr] = vis.map(fn(p) { make_ident_expr(p.0) })
  get_args.push(make_ident_expr("config"))
  let body = make_let(
    "client",
    make_apply(make_method_ref(client_name, "get_with_config"), get_args),
    make_defer(
      make_dot_apply(make_ident_expr("client"), "drop", []),
      make_apply(make_ident_expr("f"), [make_ident_expr("client")]),
    ),
  )
  make_top_func_generic(
    Some(client_name),
    "scoped_with_config",
    ["T"],
    params,
    Some(make_type("T")),
    true,
    body,
  )
}

///|
fn build_client_scoped_new_phantom_with_config(
  agent : AgentInfo,
  client_name : String,
  override_name : String,
) -> @syntax.Impl {
  let vis = visible_params(agent.constructor_params)
  let params : Array[@syntax.Parameter] = vis.map(fn(p) {
    make_typed_param(p.0, param_type_to_ast_type(p.1))
  })
  params.push(make_typed_param("config", make_type(override_name)))
  params.push(make_typed_param("f", make_callback_type(client_name)))
  let args : Array[@syntax.Expr] = vis.map(fn(p) { make_ident_expr(p.0) })
  args.push(make_ident_expr("config"))
  let body = make_let(
    "client",
    make_apply(make_method_ref(client_name, "new_phantom_with_config"), args),
    make_defer(
      make_dot_apply(make_ident_expr("client"), "drop", []),
      make_apply(make_ident_expr("f"), [make_ident_expr("client")]),
    ),
  )
  make_top_func_generic(
    Some(client_name),
    "scoped_new_phantom_with_config",
    ["T"],
    params,
    Some(make_type("T")),
    true,
    body,
  )
}

///|
fn build_client_scoped_get_phantom_with_config(
  agent : AgentInfo,
  client_name : String,
  override_name : String,
) -> @syntax.Impl {
  let vis = visible_params(agent.constructor_params)
  let params : Array[@syntax.Parameter] = vis.map(fn(p) {
    make_typed_param(p.0, param_type_to_ast_type(p.1))
  })
  params.push(
    make_typed_param(
      "phantom_id",
      make_qualified_parameterized_type("rpcTypes", "Uuid", []),
    ),
  )
  params.push(make_typed_param("config", make_type(override_name)))
  params.push(make_typed_param("f", make_callback_type(client_name)))
  let args : Array[@syntax.Expr] = vis.map(fn(p) { make_ident_expr(p.0) })
  args.push(make_ident_expr("phantom_id"))
  args.push(make_ident_expr("config"))
  let body = make_let(
    "client",
    make_apply(make_method_ref(client_name, "get_phantom_with_config"), args),
    make_defer(
      make_dot_apply(make_ident_expr("client"), "drop", []),
      make_apply(make_ident_expr("f"), [make_ident_expr("client")]),
    ),
  )
  make_top_func_generic(
    Some(client_name),
    "scoped_get_phantom_with_config",
    ["T"],
    params,
    Some(make_type("T")),
    true,
    body,
  )
}
