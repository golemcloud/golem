///|
pub fn param_type_to_type_string(pt : ParamType) -> String {
  match pt {
    Simple(name) => name
    Optional(inner) => "\{param_type_to_type_string(inner)}?"
    List(inner) => "Array[\{param_type_to_type_string(inner)}]"
    ResultType(ok, err) =>
      "Result[\{param_type_to_type_string(ok)}, \{param_type_to_type_string(err)}]"
  }
}

///|
fn param_type_to_ast_type(pt : ParamType) -> @syntax.Type {
  match pt {
    Simple(name) => make_type(name)
    Optional(inner) => make_option_type(param_type_to_ast_type(inner))
    List(inner) =>
      make_parameterized_type("Array", [param_type_to_ast_type(inner)])
    ResultType(ok, err) =>
      make_parameterized_type("Result", [
        param_type_to_ast_type(ok),
        param_type_to_ast_type(err),
      ])
  }
}

///|
fn param_type_to_default_ast_expr(pt : ParamType) -> @syntax.Expr {
  match pt {
    Simple("String") => make_string_expr("")
    Simple("Bool") => make_bool_expr(false)
    Simple("Int") => make_int_expr(0)
    Simple("UInt") =>
      @syntax.Expr::Constant(c=@syntax.Constant::UInt("0"), loc=dummy_loc)
    Simple("Int64") =>
      @syntax.Expr::Constant(c=@syntax.Constant::Int64("0"), loc=dummy_loc)
    Simple("UInt64") =>
      @syntax.Expr::Constant(c=@syntax.Constant::UInt64("0"), loc=dummy_loc)
    Simple("Float") =>
      @syntax.Expr::Constant(c=@syntax.Constant::Float("0.0"), loc=dummy_loc)
    Simple("Double") =>
      @syntax.Expr::Constant(c=@syntax.Constant::Double("0.0"), loc=dummy_loc)
    Simple("Byte") =>
      @syntax.Expr::Constant(c=@syntax.Constant::Byte("\\x00"), loc=dummy_loc)
    Simple("Char") =>
      @syntax.Expr::Constant(c=@syntax.Constant::Char("\\x00"), loc=dummy_loc)
    Simple(_) => make_apply(make_ident_expr("default"), [])
    Optional(_) => make_constr_no_args("None")
    List(_) => make_array([])
    ResultType(ok, _) =>
      make_apply(make_constr_no_args("Ok"), [param_type_to_default_ast_expr(ok)])
  }
}

///|
pub fn param_type_to_schema_expr(pt : ParamType) -> @syntax.Expr {
  let default_expr = param_type_to_default_ast_expr(pt)
  let ty = param_type_to_ast_type(pt)
  make_apply(make_qualified_expr("schema", "schema_of"), [
    make_constraint_expr(default_expr, ty),
  ])
}

///|
fn build_construct_fn(agent : AgentInfo) -> @syntax.Expr {
  let param_count = agent.constructor_params.length()
  // Ok(AgentName::new(p1, p2, ...))
  let new_args = agent.constructor_params.map(fn(param) {
    make_ident_expr(param.0)
  })
  let ok_call = make_apply(make_constr_no_args("Ok"), [
    make_apply(make_method_ref(agent.name, "new"), new_args),
  ])
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
  let try_body = make_let(
    "elements",
    make_apply(make_qualified_expr("extractor", "extract_tuple"), [
      make_ident_expr("input"),
    ]),
    body,
  )
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
fn build_register_call(agent : AgentInfo) -> @syntax.Expr {
  let schema_elems = agent.constructor_params.map(fn(param) {
    let (name, pt) = param
    make_tuple([make_string_expr(name), param_type_to_schema_expr(pt)])
  })
  let input_schema = make_apply(
    make_qualified_constr("common", "DataSchema", "Tuple"),
    [make_array(schema_elems)],
  )
  let constructor_record = make_record([
    make_field("name", make_constr_no_args("None")),
    make_field("description", make_string_expr(agent.constructor_description)),
    make_field("prompt_hint", make_constr_no_args("None")),
    make_field("input_schema", input_schema),
  ])
  let agent_type = make_record([
    make_field("type_name", make_string_expr(agent.name)),
    make_field("description", make_string_expr(agent.description)),
    make_field("constructor_", constructor_record),
    make_field("methods", make_array([])),
    make_field("dependencies", make_array([])),
  ])
  make_apply(make_qualified_expr("agents", "register_agent"), [
    make_record([
      make_field("name", make_string_expr(agent.name)),
      make_field("agent_type", agent_type),
      make_field("construct", build_construct_fn(agent)),
    ]),
  ])
}

///|
pub fn generate_agent_registration(
  agents : Array[AgentInfo],
) -> @list.List[@syntax.Impl] {
  let calls = agents.map(fn(a) { build_register_call(a) })
  to_list([make_init_block(make_sequence(calls))])
}
