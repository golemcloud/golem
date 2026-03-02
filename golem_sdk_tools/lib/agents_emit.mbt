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
      "@types.Multimodal[\{param_type_to_type_string(inner)}]"
    Parameterized(name, params) => {
      let param_strs = params.map(param_type_to_type_string)
      let joined = param_strs.join(", ")
      "\{name}[\{joined}]"
    }
  }
}

///|
/// Converts a `ParamType` to a MoonBit syntax AST `Type` node.
fn param_type_to_ast_type(pt : ParamType) -> @syntax.Type {
  match pt {
    Simple(name) =>
      match decode_qualified(name) {
        Some((pkg, id)) => make_qualified_parameterized_type(pkg, id, [])
        None => make_type(name)
      }
    Optional(inner) => make_option_type(param_type_to_ast_type(inner))
    List(inner) =>
      make_parameterized_type("Array", [param_type_to_ast_type(inner)])
    ResultType(ok, err) =>
      make_parameterized_type("Result", [
        param_type_to_ast_type(ok),
        param_type_to_ast_type(err),
      ])
    Tuple(elems) => make_tuple_type(elems.map(param_type_to_ast_type))
    MultimodalType(inner) =>
      make_qualified_parameterized_type("types", "Multimodal", [
        param_type_to_ast_type(inner),
      ])
    Parameterized(name, params) => {
      let ast_params = params.map(param_type_to_ast_type)
      match decode_qualified(name) {
        Some((pkg, id)) =>
          make_qualified_parameterized_type(pkg, id, ast_params)
        None => make_parameterized_type(name, ast_params)
      }
    }
  }
}

///|
/// Generates an AST expression that produces the `ElementSchema` for a `ParamType`.
pub fn param_type_to_schema_expr(
  pt : ParamType,
  options? : SchemaOptions? = None,
) -> @syntax.Expr {
  match pt {
    Tuple(elems) =>
      make_apply(make_qualified_expr("schema", "make_tuple_schema"), [
        make_array(elems.map(fn(e) { param_type_to_schema_expr(e) })),
      ])
    _ => {
      let ty = param_type_to_ast_type(pt)
      let type_tag_ty = make_qualified_parameterized_type("schema", "TypeTag", [
        ty,
      ])
      let tag_expr = make_constraint_expr(
        make_record_with_type("schema", "TypeTag", []),
        type_tag_ty,
      )
      match options {
        Some(opts) if opts.text_languages.length() > 0 ||
          opts.binary_mime_types.length() > 0 => {
          let options_expr = make_record_with_type("schema", "SchemaOptions", [
            make_field(
              "text_languages",
              make_array(opts.text_languages.map(make_string_expr)),
            ),
            make_field(
              "binary_mime_types",
              make_array(opts.binary_mime_types.map(make_string_expr)),
            ),
          ])
          make_apply(
            make_qualified_expr("schema", "schema_of_tag_with_options"),
            [tag_expr, options_expr],
          )
        }
        _ =>
          make_apply(make_qualified_expr("schema", "schema_of_tag"), [tag_expr])
      }
    }
  }
}

///|
/// Generates an AST expression that produces the multimodal schema for a `MultimodalType` parameter.
fn build_multimodal_schema_expr(pt : ParamType) -> @syntax.Expr {
  match pt {
    MultimodalType(inner) => {
      let ty = param_type_to_ast_type(inner)
      let type_tag_ty = make_qualified_parameterized_type("schema", "TypeTag", [
        ty,
      ])
      let tag_expr = make_constraint_expr(
        make_record_with_type("schema", "TypeTag", []),
        type_tag_ty,
      )
      make_apply(make_qualified_expr("types", "multimodal_schema_of_tag"), [
        tag_expr,
      ])
    }
    _ => panic()
  }
}

///|
/// Generates the constructor lambda expression for an agent's `register_agent` call.
fn build_construct_fn(agent : AgentInfo) -> @syntax.Expr {
  let is_multimodal_ctor = agent.constructor_params.length() == 1 &&
    is_multimodal_type(agent.constructor_params[0].1)
  let try_body = if is_multimodal_ctor {
    build_multimodal_construct_body(agent)
  } else {
    build_tuple_construct_body(agent)
  }
  // fn(input) { try { ... } catch { e => Err(e) } }
  make_lambda(
    ["input"],
    make_try_catch(try_body, [
      make_case(
        make_var_pattern("e"),
        make_apply(make_constr_no_args("Err"), [make_ident_expr("e")]),
      ),
    ]),
  )
}

///|
/// Generates the constructor body for agents with tuple-style (non-multimodal) parameters.
fn build_tuple_construct_body(agent : AgentInfo) -> @syntax.Expr {
  let param_count = agent.constructor_params.length()
  let new_args = agent.constructor_params.map(fn(param) {
    make_ident_expr(param.0)
  })
  let new_call = make_apply(make_method_ref(agent.name, "new"), new_args)
  let snapshottable_expr = if agent.has_snapshottable {
    make_apply(make_constr_no_args("Some"), [make_ident_expr("agent")])
  } else {
    make_constr_no_args("None")
  }
  let constructed = make_record_with_type("agents", "ConstructedAgent", [
    make_field("raw", make_ident_expr("agent")),
    make_field("snapshottable", snapshottable_expr),
  ])
  let ok_call = make_let(
    "agent",
    new_call,
    make_let(
      "constructed",
      constructed,
      make_apply(make_constr_no_args("Ok"), [make_ident_expr("constructed")]),
    ),
  )
  // Build let bindings for each parameter (from last to first)
  let mut body = ok_call
  for idx in 0..<param_count {
    let i = param_count - 1 - idx
    let (name, pt) = agent.constructor_params[i]
    body = make_let_typed(
      name,
      param_type_to_ast_type(pt),
      make_apply(make_qualified_expr("schema", "from_element_value_as"), [
        make_array_get(make_ident_expr("elements"), make_int_expr(i)),
      ]),
      body,
    )
  }
  // guard elements.length() == N else { raise ... }
  let elements_length = make_dot_apply(make_ident_expr("elements"), "length", [])
  body = make_guard_else(
    make_infix("==", elements_length, make_int_expr(param_count)),
    make_raise(
      make_apply(
        make_qualified_constr("common", "AgentError", "InvalidInput"),
        [
          make_infix(
            "+",
            make_string_expr("Expected \{param_count} elements, got "),
            make_dot_apply(
              make_dot_apply(make_ident_expr("elements"), "length", []),
              "to_string",
              [],
            ),
          ),
        ],
      ),
    ),
    body,
  )
  // let elements = @extractor.extract_tuple(input); <guard + lets>
  make_let(
    "elements",
    make_apply(make_qualified_expr("extractor", "extract_tuple"), [
      make_ident_expr("input"),
    ]),
    body,
  )
}

///|
/// Generates the constructor body for agents with a single multimodal parameter.
fn build_multimodal_construct_body(agent : AgentInfo) -> @syntax.Expr {
  let (param_name, param_type) = agent.constructor_params[0]
  let new_call = make_apply(make_method_ref(agent.name, "new"), [
    make_ident_expr(param_name),
  ])
  let snapshottable_expr = if agent.has_snapshottable {
    make_apply(make_constr_no_args("Some"), [make_ident_expr("agent")])
  } else {
    make_constr_no_args("None")
  }
  let constructed = make_record_with_type("agents", "ConstructedAgent", [
    make_field("raw", make_ident_expr("agent")),
    make_field("snapshottable", snapshottable_expr),
  ])
  let ok_call = make_let(
    "agent",
    new_call,
    make_let(
      "constructed",
      constructed,
      make_apply(make_constr_no_args("Ok"), [make_ident_expr("constructed")]),
    ),
  )
  // let param_name : Type = @types.Multimodal::from_data_value(input)
  make_let_typed(
    param_name,
    param_type_to_ast_type(param_type),
    make_apply(
      make_qualified_method_ref("types", "Multimodal", "from_data_value"),
      [make_ident_expr("input")],
    ),
    ok_call,
  )
}

///|
/// Generates an AST expression for an `AgentMethod` record literal.
fn build_method_record(info : MethodInfo) -> @syntax.Expr {
  let is_multimodal_input = info.params.length() == 1 &&
    is_multimodal_type(info.params[0].1)
  let input_schema = if is_multimodal_input {
    make_apply(make_qualified_constr("common", "DataSchema", "Multimodal"), [
      build_multimodal_schema_expr(info.params[0].1),
    ])
  } else {
    let input_elems = info.params.map(fn(param) {
      let (name, pt) = param
      let opts = info.param_schema_options.get(name)
      make_tuple([
        make_string_expr(name),
        param_type_to_schema_expr(pt, options=opts),
      ])
    })
    make_apply(make_qualified_constr("common", "DataSchema", "Tuple"), [
      make_array(input_elems),
    ])
  }
  let is_multimodal_output = match info.return_type {
    Some(pt) => is_multimodal_type(pt)
    None => false
  }
  let output_schema = if is_multimodal_output {
    make_apply(make_qualified_constr("common", "DataSchema", "Multimodal"), [
      build_multimodal_schema_expr(info.return_type.unwrap()),
    ])
  } else {
    match info.return_type {
      Some(pt) =>
        make_apply(make_qualified_constr("common", "DataSchema", "Tuple"), [
          make_array([
            make_tuple([
              make_string_expr("result"),
              param_type_to_schema_expr(pt),
            ]),
          ]),
        ])
      None =>
        make_apply(make_qualified_constr("common", "DataSchema", "Tuple"), [
          make_array([]),
        ])
    }
  }
  let hint_expr = match info.prompt_hint {
    Some(s) => make_apply(make_constr_no_args("Some"), [make_string_expr(s)])
    None => make_constr_no_args("None")
  }
  make_record([
    make_field("name", make_string_expr(info.name)),
    make_field("description", make_string_expr(info.description)),
    make_field("prompt_hint", hint_expr),
    make_field("input_schema", input_schema),
    make_field("output_schema", output_schema),
  ])
}

///|
pub fn build_agent_type_expr(agent : AgentInfo) -> @syntax.Expr {
  let is_multimodal_ctor = agent.constructor_params.length() == 1 &&
    is_multimodal_type(agent.constructor_params[0].1)
  let input_schema = if is_multimodal_ctor {
    make_apply(make_qualified_constr("common", "DataSchema", "Multimodal"), [
      build_multimodal_schema_expr(agent.constructor_params[0].1),
    ])
  } else {
    let schema_elems = agent.constructor_params.map(fn(param) {
      let (name, pt) = param
      let opts = agent.constructor_schema_options.get(name)
      make_tuple([
        make_string_expr(name),
        param_type_to_schema_expr(pt, options=opts),
      ])
    })
    make_apply(make_qualified_constr("common", "DataSchema", "Tuple"), [
      make_array(schema_elems),
    ])
  }
  let ctor_hint_expr = match agent.constructor_prompt_hint {
    Some(s) => make_apply(make_constr_no_args("Some"), [make_string_expr(s)])
    None => make_constr_no_args("None")
  }
  let constructor_record = make_record([
    make_field(
      "name",
      make_apply(make_constr_no_args("Some"), [make_string_expr("new")]),
    ),
    make_field("description", make_string_expr(agent.constructor_description)),
    make_field("prompt_hint", ctor_hint_expr),
    make_field("input_schema", input_schema),
  ])
  let method_records = agent.methods.map(fn(m) { build_method_record(m) })
  make_record([
    make_field("type_name", make_string_expr(agent.name)),
    make_field("description", make_string_expr(agent.description)),
    make_field("constructor_", constructor_record),
    make_field("methods", make_array(method_records)),
    make_field("dependencies", make_array([])),
    make_field(
      "mode",
      make_qualified_constr(
        "common",
        "AgentMode",
        match agent.mode {
          Durable => "DURABLE"
          Ephemeral => "EPHEMERAL"
        },
      ),
    ),
  ])
}

///|
/// Generates the `register_agent(...)` call expression for a single agent.
fn build_register_call(agent : AgentInfo) -> @syntax.Expr {
  let agent_type = build_agent_type_expr(agent)
  make_apply(make_qualified_expr("agents", "register_agent"), [
    make_record([
      make_field("name", make_string_expr(agent.name)),
      make_field("agent_type", agent_type),
      make_field("construct", build_construct_fn(agent)),
    ]),
  ])
}

///|
/// Generates the match arm body for a method invocation, dispatching to tuple or multimodal handling.
fn build_method_arm_body(info : MethodInfo) -> @syntax.Expr {
  let is_multimodal_input = info.params.length() == 1 &&
    is_multimodal_type(info.params[0].1)
  let is_multimodal_output = match info.return_type {
    Some(pt) => is_multimodal_type(pt)
    None => false
  }
  if is_multimodal_input || is_multimodal_output {
    build_multimodal_method_arm_body(
      info, is_multimodal_input, is_multimodal_output,
    )
  } else {
    build_tuple_method_arm_body(info)
  }
}

///|
/// Generates the match arm body for a tuple-style method invocation with parameter deserialization and result serialization.
fn build_tuple_method_arm_body(info : MethodInfo) -> @syntax.Expr {
  let param_count = info.params.length()
  let has_return = info.return_type is Some(_)
  // Build the method call
  let method_args = info.params.map(fn(param) { make_ident_expr(param.0) })
  let call = make_dot_apply(make_ident_expr("self"), info.name, method_args)
  // Build the Ok(...) result expression
  let ok_expr = if has_return {
    make_let(
      "result",
      call,
      make_apply(make_constr_no_args("Ok"), [
        make_apply(make_qualified_constr("common", "DataValue", "Tuple"), [
          make_array([
            make_apply(make_qualified_expr("schema", "to_element_value_as"), [
              make_ident_expr("result"),
            ]),
          ]),
        ]),
      ]),
    )
  } else {
    make_sequence([
      call,
      make_apply(make_constr_no_args("Ok"), [
        make_apply(make_qualified_constr("common", "DataValue", "Tuple"), [
          make_array([]),
        ]),
      ]),
    ])
  }
  // Build let bindings for each param (from last to first)
  let mut body = ok_expr
  for idx in 0..<param_count {
    let i = param_count - 1 - idx
    let (name, pt) = info.params[i]
    body = make_let_typed(
      name,
      param_type_to_ast_type(pt),
      make_apply(make_qualified_expr("schema", "from_element_value_as"), [
        make_array_get(make_ident_expr("elements"), make_int_expr(i)),
      ]),
      body,
    )
  }
  // guard elements.length() == N
  let elements_length = make_dot_apply(make_ident_expr("elements"), "length", [])
  body = make_guard_else(
    make_infix("==", elements_length, make_int_expr(param_count)),
    make_raise(
      make_apply(
        make_qualified_constr("common", "AgentError", "InvalidInput"),
        [
          make_infix(
            "+",
            make_string_expr("Expected \{param_count} elements, got "),
            make_dot_apply(
              make_dot_apply(make_ident_expr("elements"), "length", []),
              "to_string",
              [],
            ),
          ),
        ],
      ),
    ),
    body,
  )
  // let elements = @extractor.extract_tuple(input)
  make_let(
    "elements",
    make_apply(make_qualified_expr("extractor", "extract_tuple"), [
      make_ident_expr("input"),
    ]),
    body,
  )
}

///|
/// Generates the match arm body for a method with multimodal input and/or output.
fn build_multimodal_method_arm_body(
  info : MethodInfo,
  multimodal_input : Bool,
  multimodal_output : Bool,
) -> @syntax.Expr {
  // Build the method call
  let method_args = info.params.map(fn(param) { make_ident_expr(param.0) })
  let call = make_dot_apply(make_ident_expr("self"), info.name, method_args)
  // Build result wrapping
  let has_return = info.return_type is Some(_)
  let ok_expr = if has_return && multimodal_output {
    // let result = self.method(args)
    // Ok(result.to_data_value())
    make_let(
      "result",
      call,
      make_apply(make_constr_no_args("Ok"), [
        make_dot_apply(make_ident_expr("result"), "to_data_value", []),
      ]),
    )
  } else if has_return {
    // let result = self.method(args)
    // Ok(DataValue::Tuple([to_element_value_as(result)]))
    make_let(
      "result",
      call,
      make_apply(make_constr_no_args("Ok"), [
        make_apply(make_qualified_constr("common", "DataValue", "Tuple"), [
          make_array([
            make_apply(make_qualified_expr("schema", "to_element_value_as"), [
              make_ident_expr("result"),
            ]),
          ]),
        ]),
      ]),
    )
  } else {
    // self.method(args)
    // Ok(DataValue::Tuple([]))
    make_sequence([
      call,
      make_apply(make_constr_no_args("Ok"), [
        make_apply(make_qualified_constr("common", "DataValue", "Tuple"), [
          make_array([]),
        ]),
      ]),
    ])
  }
  // Build param deserialization
  if multimodal_input {
    let (param_name, param_type) = info.params[0]
    // let param_name : Type = @types.Multimodal::from_data_value(input)
    make_let_typed(
      param_name,
      param_type_to_ast_type(param_type),
      make_apply(
        make_qualified_method_ref("types", "Multimodal", "from_data_value"),
        [make_ident_expr("input")],
      ),
      ok_expr,
    )
  } else {
    // Non-multimodal input with multimodal output — use tuple extraction as normal
    let param_count = info.params.length()
    let mut body = ok_expr
    for idx in 0..<param_count {
      let i = param_count - 1 - idx
      let (name, pt) = info.params[i]
      body = make_let_typed(
        name,
        param_type_to_ast_type(pt),
        make_apply(make_qualified_expr("schema", "from_element_value_as"), [
          make_array_get(make_ident_expr("elements"), make_int_expr(i)),
        ]),
        body,
      )
    }
    let elements_length = make_dot_apply(
      make_ident_expr("elements"),
      "length",
      [],
    )
    body = make_guard_else(
      make_infix("==", elements_length, make_int_expr(param_count)),
      make_raise(
        make_apply(
          make_qualified_constr("common", "AgentError", "InvalidInput"),
          [
            make_infix(
              "+",
              make_string_expr("Expected \{param_count} elements, got "),
              make_dot_apply(
                make_dot_apply(make_ident_expr("elements"), "length", []),
                "to_string",
                [],
              ),
            ),
          ],
        ),
      ),
      body,
    )
    make_let(
      "elements",
      make_apply(make_qualified_expr("extractor", "extract_tuple"), [
        make_ident_expr("input"),
      ]),
      body,
    )
  }
}

///|
/// Generates the `impl RawAgent for AgentName` AST node with method dispatch logic.
pub fn build_raw_agent_impl(agent : AgentInfo) -> @syntax.Impl {
  // Build match arms for each method
  let cases : Array[@syntax.Case] = agent.methods.map(fn(m) {
    make_case(make_string_pattern(m.name), build_method_arm_body(m))
  })
  // Default arm: other => Err(@common.AgentError::InvalidMethod(other))
  cases.push(
    make_case(
      make_var_pattern("other"),
      make_apply(make_constr_no_args("Err"), [
        make_apply(
          make_qualified_constr("common", "AgentError", "InvalidMethod"),
          [make_ident_expr("other")],
        ),
      ]),
    ),
  )
  let match_expr = make_match(make_ident_expr("method_name"), cases)
  let try_body = make_try_catch(match_expr, [
    make_case(
      make_var_pattern("e"),
      make_apply(make_constr_no_args("Err"), [make_ident_expr("e")]),
    ),
  ])
  make_top_impl(
    agent.name,
    "agents",
    "RawAgent",
    "invoke",
    ["self", "method_name", "input"],
    try_body,
  )
}

///|
/// Generates all agent registration code: the `init` block calling
/// `register_agent` and all `RawAgent` impls.
pub fn generate_agent_registration(
  agents : Array[AgentInfo],
) -> @list.List[@syntax.Impl] {
  let calls = agents.map(fn(a) { build_register_call(a) })
  let impls = agents.map(fn(a) { build_raw_agent_impl(a) })
  let needs_try_catch = agents
    .iter()
    .any(fn(a) {
      a.constructor_schema_options
      .iter()
      .any(fn(kv) {
        kv.1.text_languages.length() > 0 || kv.1.binary_mime_types.length() > 0
      }) ||
      a.methods
      .iter()
      .any(fn(m) {
        m.param_schema_options
        .iter()
        .any(fn(kv) {
          kv.1.text_languages.length() > 0 ||
          kv.1.binary_mime_types.length() > 0
        })
      })
    })
  let init_body = if needs_try_catch {
    make_try_catch(make_sequence(calls), [
      make_case(
        make_var_pattern("e"),
        make_apply(make_ident_expr("abort"), [
          make_dot_apply(make_ident_expr("e"), "to_string", []),
        ]),
      ),
    ])
  } else {
    make_sequence(calls)
  }
  let all : Array[@syntax.Impl] = [make_init_block(init_body)]
  for impl_ in impls {
    all.push(impl_)
  }
  to_list(all)
}
