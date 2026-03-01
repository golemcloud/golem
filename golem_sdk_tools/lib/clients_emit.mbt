///|
/// Checks if any constructor parameter uses an RPC-incompatible type.
fn has_rpc_incompatible_constructor(agent : AgentInfo) -> Bool {
  agent.constructor_params
  .iter()
  .find_first(fn(p) { is_rpc_incompatible_type(p.1) })
  is Some(_)
}

///|
/// Checks if any parameter or return type of a method uses an RPC-incompatible type.
fn method_has_rpc_incompatible_types(meth : MethodInfo) -> Bool {
  for p in meth.params {
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
) -> @list.List[@syntax.Impl] {
  let impls : Array[@syntax.Impl] = []
  for agent in agents {
    // Skip agents with RPC-incompatible constructors — Multimodal and
    // UnstructuredText/UnstructuredBinary types can't be serialized to WitValue
    // for RPC transport
    if has_rpc_incompatible_constructor(agent) {
      continue
    }
    let client_name = agent.name + "Client"
    impls.push(build_client_struct(client_name))
    impls.push(build_client_get(agent, client_name))
    impls.push(build_client_new_phantom(agent, client_name))
    impls.push(build_client_get_agent_id(client_name))
    impls.push(build_client_phantom_id(client_name))
    impls.push(build_client_drop(client_name))
    for meth in agent.methods {
      // Skip methods with RPC-incompatible params/return types
      if method_has_rpc_incompatible_types(meth) {
        continue
      }
      impls.push(build_client_method_invoke(agent, meth, client_name))
      impls.push(build_client_method_trigger(agent, meth, client_name))
      impls.push(build_client_method_schedule(agent, meth, client_name))
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
fn build_constructor_data_expr(agent : AgentInfo) -> @syntax.Expr {
  // Check if the constructor has a multimodal parameter (must be the only param)
  let has_multimodal = agent.constructor_params
    .iter()
    .find_first(fn(p) { is_multimodal_type(p.1) })
  if has_multimodal is Some(_) {
    // Multimodal constructor: param.to_data_value()
    let param_name = agent.constructor_params[0].0
    make_dot_apply(make_ident_expr(param_name), "to_data_value", [])
  } else {
    let elements = agent.constructor_params.map(fn(param) {
      make_apply(make_qualified_expr("schema", "to_element_value_as"), [
        make_ident_expr(param.0),
      ])
    })
    make_apply(make_qualified_constr("common", "DataValue", "Tuple"), [
      make_array(elements),
    ])
  }
}

///|
/// Generates the `get` constructor that creates a client by initializing the remote agent.
fn build_client_get(agent : AgentInfo, client_name : String) -> @syntax.Impl {
  let params : Array[@syntax.Parameter] = agent.constructor_params.map(fn(p) {
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
/// Generates the `new_phantom` constructor that creates a phantom client with a given UUID.
fn build_client_new_phantom(
  agent : AgentInfo,
  client_name : String,
) -> @syntax.Impl {
  let params : Array[@syntax.Parameter] = agent.constructor_params.map(fn(p) {
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
        make_qualified_method_ref("rpc", "AgentClient", "new_phantom"),
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
    "new_phantom",
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
    false,
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
    true,
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
/// Generates the expression that serializes a single method parameter to a `WitValue` for RPC.
fn build_rpc_param_expr(param : (String, ParamType)) -> @syntax.Expr {
  let (name, pt) = param
  if is_multimodal_type(pt) {
    make_apply(make_qualified_expr("rpc", "multimodal_to_wit_value"), [
      make_ident_expr(name),
    ])
  } else if is_unstructured_text_type(pt) {
    make_apply(make_qualified_expr("rpc", "unstructured_text_to_wit_value"), [
      make_ident_expr(name),
    ])
  } else if is_unstructured_binary_type(pt) {
    make_apply(make_qualified_expr("rpc", "unstructured_binary_to_wit_value"), [
      make_ident_expr(name),
    ])
  } else {
    make_apply(make_qualified_expr("rpc", "to_wit_value"), [
      make_ident_expr(name),
    ])
  }
}

///|
/// Generates the array expression containing all serialized method parameters for RPC.
fn build_rpc_params_expr(meth : MethodInfo) -> @syntax.Expr {
  make_array(meth.params.map(build_rpc_param_expr))
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
  let has_params = meth.params.length() > 0
  let params : Array[@syntax.Parameter] = [make_self_param(client_name)]
  for p in meth.params {
    params.push(make_typed_param(p.0, param_type_to_ast_type(p.1)))
  }
  let rpc_params = build_rpc_params_expr(meth)
  let invoke_expr = make_dot_apply(
    make_field_access(make_ident_expr("self"), "client"),
    "invoke_and_await",
    [make_string_expr(rpc_name), rpc_params],
  )
  let body = if has_return {
    let unwrapped = make_apply(make_qualified_expr("rpc", "unwrap_wit_tuple"), [
      make_ident_expr("result"),
    ])
    let decode_expr = match meth.return_type {
      Some(rt) =>
        if is_multimodal_type(rt) {
          make_apply(make_qualified_expr("rpc", "wit_value_to_multimodal"), [
            unwrapped,
          ])
        } else if is_unstructured_text_type(rt) {
          make_apply(
            make_qualified_expr("rpc", "wit_value_to_unstructured_text"),
            [unwrapped],
          )
        } else if is_unstructured_binary_type(rt) {
          make_apply(
            make_qualified_expr("rpc", "wit_value_to_unstructured_binary"),
            [unwrapped],
          )
        } else {
          make_apply(make_qualified_expr("rpc", "from_wit_value"), [unwrapped])
        }
      None =>
        make_apply(make_qualified_expr("rpc", "from_wit_value"), [unwrapped])
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
    has_return || has_params,
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
  let has_params = meth.params.length() > 0
  let params : Array[@syntax.Parameter] = [make_self_param(client_name)]
  for p in meth.params {
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
  let has_params = meth.params.length() > 0
  let params : Array[@syntax.Parameter] = [make_self_param(client_name)]
  params.push(
    make_typed_param(
      "scheduled_at",
      make_qualified_parameterized_type("wallClock", "Datetime", []),
    ),
  )
  for p in meth.params {
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
