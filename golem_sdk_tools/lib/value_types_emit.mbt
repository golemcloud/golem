///|
pub fn generate_value_type_impls(
  types : Array[ValueTypeInfo],
) -> @list.List[@syntax.Impl] {
  let all : Array[@syntax.Impl] = []
  for t in types {
    match t.kind {
      Record(fields) => {
        all.push(build_record_has_element_schema(t.name, fields))
        all.push(build_record_from_extractor(t.name, fields))
        all.push(build_from_element_value(t.name))
        all.push(build_record_to_element_value(t.name, fields))
      }
      SimpleEnum(cases) => {
        all.push(build_enum_has_element_schema(t.name, cases))
        all.push(build_enum_from_extractor(t.name, cases))
        all.push(build_from_element_value(t.name))
        all.push(build_enum_to_element_value(t.name, cases))
      }
      VariantEnum(cases) => {
        all.push(build_variant_has_element_schema(t.name, cases))
        all.push(build_variant_from_extractor(t.name, cases))
        all.push(build_from_element_value(t.name))
        all.push(build_variant_to_element_value(t.name, cases))
      }
    }
  }
  to_list(all)
}

// --- Shared helpers ---

///|
fn make_pun_field(name : String) -> @syntax.FieldDef {
  @syntax.FieldDef::{
    label: make_label(name),
    expr: make_ident_expr(name),
    is_pun: true,
    loc: dummy_loc,
  }
}

///|
fn make_int_pattern(n : Int) -> @syntax.Pattern {
  @syntax.Pattern::Constant(c=@syntax.Constant::Int(n.to_string()), loc=dummy_loc)
}

///|
fn make_wildcard_pattern() -> @syntax.Pattern {
  @syntax.Pattern::Any(loc=dummy_loc)
}

///|
fn make_constr_pattern(
  name : String,
  args : Array[@syntax.Pattern],
) -> @syntax.Pattern {
  let constr = @syntax.Constructor::{
    name: @syntax.ConstrName::{ name, loc: dummy_loc },
    extra_info: @syntax.ConstructorExtraInfo::NoExtraInfo,
    loc: dummy_loc,
  }
  if args.is_empty() {
    @syntax.Pattern::Constr(constr~, args=None, is_open=false, loc=dummy_loc)
  } else {
    let pat_args = args.map(fn(p) {
      @syntax.ConstrPatArg::{
        pat: p,
        kind: @syntax.ArgumentKind::Positional,
      }
    })
    @syntax.Pattern::Constr(
      constr~,
      args=Some(to_list(pat_args)),
      is_open=false,
      loc=dummy_loc,
    )
  }
}

///|
fn make_uint_expr(n : Int) -> @syntax.Expr {
  @syntax.Expr::Constant(
    c=@syntax.Constant::UInt(n.to_string()),
    loc=dummy_loc,
  )
}

///|
fn build_from_element_value(type_name : String) -> @syntax.Impl {
  // @schema.FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
  let body = make_apply(
    make_qualified_expr("schema", "from_extractor_as"),
    [
      make_apply(
        make_qualified_expr("extractor", "extract_component_model_value"),
        [make_ident_expr("v")],
      ),
    ],
  )
  make_top_impl(
    type_name,
    "schema",
    "FromElementValue",
    "from_element_value",
    ["v"],
    body,
  )
}

// --- Record ---

///|
fn build_record_has_element_schema(
  type_name : String,
  fields : Array[(String, ParamType)],
) -> @syntax.Impl {
  let field_exprs = fields.map(fn(field) {
    let (name, pt) = field
    make_tuple([make_string_expr(name), param_type_to_schema_expr(pt)])
  })
  let body = make_apply(
    make_qualified_expr("schema", "make_record_schema"),
    [make_array(field_exprs)],
  )
  make_top_impl(
    type_name,
    "schema",
    "HasElementSchema",
    "element_schema",
    [],
    body,
  )
}

///|
fn build_record_from_extractor(
  type_name : String,
  fields : Array[(String, ParamType)],
) -> @syntax.Impl {
  // Build record construction { field1, field2, ... }
  let record_fields = fields.map(fn(field) { make_pun_field(field.0) })
  let mut body : @syntax.Expr = make_record(record_fields)
  // Build let bindings from last to first
  for idx in 0..<fields.length() {
    let i = fields.length() - 1 - idx
    let (name, pt) = fields[i]
    body = make_let_typed(
      name,
      param_type_to_ast_type(pt),
      make_apply(make_qualified_expr("schema", "extract_field"), [
        make_ident_expr("e"),
        make_int_expr(i),
      ]),
      body,
    )
  }
  make_top_impl(
    type_name,
    "schema",
    "FromExtractor",
    "from_extractor",
    ["e"],
    body,
  )
}

///|
fn build_record_to_element_value(
  type_name : String,
  fields : Array[(String, ParamType)],
) -> @syntax.Impl {
  let field_exprs = fields.map(fn(field) {
    let (name, _pt) = field
    make_apply(make_qualified_expr("schema", "to_element_value_as"), [
      make_field_access(make_ident_expr("self"), name),
    ])
  })
  let body = make_apply(
    make_qualified_expr("schema", "make_record_value"),
    [make_array(field_exprs)],
  )
  make_top_impl(
    type_name,
    "schema",
    "ToElementValue",
    "to_element_value",
    ["self"],
    body,
  )
}

// --- SimpleEnum ---

///|
fn build_enum_has_element_schema(
  type_name : String,
  cases : Array[String],
) -> @syntax.Impl {
  let case_exprs = cases.map(fn(name) { make_string_expr(name) })
  let body = make_apply(
    make_qualified_expr("schema", "make_enum_schema"),
    [make_array(case_exprs)],
  )
  make_top_impl(
    type_name,
    "schema",
    "HasElementSchema",
    "element_schema",
    [],
    body,
  )
}

///|
fn build_enum_from_extractor(
  type_name : String,
  cases : Array[String],
) -> @syntax.Impl {
  // match idx { 0 => Low, 1 => Medium, ... _ => raise ... }
  let match_cases : Array[@syntax.Case] = []
  for i, name in cases {
    match_cases.push(
      make_case(make_int_pattern(i), make_constr_no_args(name)),
    )
  }
  // Wildcard case
  match_cases.push(
    make_case(
      make_wildcard_pattern(),
      make_raise(
        make_apply(
          make_qualified_constr("common", "AgentError", "InvalidInput"),
          [
            make_infix(
              "+",
              make_string_expr("Unknown \{type_name} case: "),
              make_dot_apply(make_ident_expr("idx"), "to_string", []),
            ),
          ],
        ),
      ),
    ),
  )
  let match_expr = make_match(make_ident_expr("idx"), match_cases)
  let body = make_let(
    "idx",
    make_apply(make_qualified_expr("schema", "extract_enum"), [
      make_ident_expr("e"),
    ]),
    match_expr,
  )
  make_top_impl(
    type_name,
    "schema",
    "FromExtractor",
    "from_extractor",
    ["e"],
    body,
  )
}

///|
fn build_enum_to_element_value(
  type_name : String,
  cases : Array[String],
) -> @syntax.Impl {
  let match_cases : Array[@syntax.Case] = []
  for i, name in cases {
    match_cases.push(
      make_case(
        make_constr_pattern(name, []),
        make_apply(make_qualified_expr("schema", "make_enum_value"), [
          make_uint_expr(i),
        ]),
      ),
    )
  }
  let body = make_match(make_ident_expr("self"), match_cases)
  make_top_impl(
    type_name,
    "schema",
    "ToElementValue",
    "to_element_value",
    ["self"],
    body,
  )
}

// --- VariantEnum ---

///|
fn variant_payload_schema_expr(payload : VariantPayload) -> @syntax.Expr {
  match payload {
    None => make_constr_no_args("None")
    Single(pt) =>
      make_apply(make_constr_no_args("Some"), [param_type_to_schema_expr(pt)])
    Multi(fields) => {
      let field_exprs = fields.map(fn(f) {
        let (name, pt) = f
        make_tuple([make_string_expr(name), param_type_to_schema_expr(pt)])
      })
      make_apply(make_constr_no_args("Some"), [
        make_apply(make_qualified_expr("schema", "make_record_schema"), [
          make_array(field_exprs),
        ]),
      ])
    }
  }
}

///|
fn build_variant_has_element_schema(
  type_name : String,
  cases : Array[(String, VariantPayload)],
) -> @syntax.Impl {
  let case_exprs = cases.map(fn(c) {
    let (name, payload) = c
    make_tuple([make_string_expr(name), variant_payload_schema_expr(payload)])
  })
  let body = make_apply(
    make_qualified_expr("schema", "make_variant_schema"),
    [make_array(case_exprs)],
  )
  make_top_impl(
    type_name,
    "schema",
    "HasElementSchema",
    "element_schema",
    [],
    body,
  )
}

///|
fn build_variant_from_extractor_case(
  type_name : String,
  idx : Int,
  case_name : String,
  payload : VariantPayload,
) -> @syntax.Case {
  match payload {
    None => make_case(make_int_pattern(idx), make_constr_no_args(case_name))
    Single(pt) => {
      // guard payload is Some(p) else { raise ... }
      // let field0 : T = @schema.FromExtractor::from_extractor(p)
      // CaseName(field0)
      let result = make_apply(make_constr_no_args(case_name), [
        make_ident_expr("field0"),
      ])
      let body = make_let_typed(
        "field0",
        param_type_to_ast_type(pt),
        make_apply(make_qualified_expr("schema", "from_extractor_as"), [
          make_ident_expr("p"),
        ]),
        result,
      )
      let guarded = make_guard_else(
        @syntax.Expr::Is(
          expr=make_ident_expr("payload"),
          pat=make_constr_pattern("Some", [make_var_pattern("p")]),
          loc=dummy_loc,
        ),
        make_raise(
          make_apply(
            make_qualified_constr("common", "AgentError", "InvalidInput"),
            [
              make_string_expr(
                "Expected payload for \{type_name}::\{case_name}",
              ),
            ],
          ),
        ),
        body,
      )
      make_case(make_int_pattern(idx), guarded)
    }
    Multi(fields) => {
      // guard payload is Some(p) else { raise ... }
      // let field0 : T0 = @schema.extract_field(p, 0)
      // let field1 : T1 = @schema.extract_field(p, 1)
      // CaseName(field0, field1)
      let field_args = fields.mapi(fn(i, _f) {
        make_ident_expr("field\{i}")
      })
      let result = make_apply(make_constr_no_args(case_name), field_args)
      let mut body = result
      for rev_idx in 0..<fields.length() {
        let i = fields.length() - 1 - rev_idx
        let (_name, pt) = fields[i]
        body = make_let_typed(
          "field\{i}",
          param_type_to_ast_type(pt),
          make_apply(make_qualified_expr("schema", "extract_field"), [
            make_ident_expr("p"),
            make_int_expr(i),
          ]),
          body,
        )
      }
      let guarded = make_guard_else(
        @syntax.Expr::Is(
          expr=make_ident_expr("payload"),
          pat=make_constr_pattern("Some", [make_var_pattern("p")]),
          loc=dummy_loc,
        ),
        make_raise(
          make_apply(
            make_qualified_constr("common", "AgentError", "InvalidInput"),
            [
              make_string_expr(
                "Expected payload for \{type_name}::\{case_name}",
              ),
            ],
          ),
        ),
        body,
      )
      make_case(make_int_pattern(idx), guarded)
    }
  }
}

///|
fn build_variant_from_extractor(
  type_name : String,
  cases : Array[(String, VariantPayload)],
) -> @syntax.Impl {
  let match_cases : Array[@syntax.Case] = []
  for i, c in cases {
    let (name, payload) = c
    match_cases.push(
      build_variant_from_extractor_case(type_name, i, name, payload),
    )
  }
  // Wildcard case
  match_cases.push(
    make_case(
      make_wildcard_pattern(),
      make_raise(
        make_apply(
          make_qualified_constr("common", "AgentError", "InvalidInput"),
          [make_string_expr("Unknown \{type_name} case")],
        ),
      ),
    ),
  )
  let match_expr = make_match(make_ident_expr("idx"), match_cases)
  let body = make_let_tuple(
    ["idx", "payload"],
    make_apply(make_qualified_expr("schema", "extract_variant"), [
      make_ident_expr("e"),
    ]),
    match_expr,
  )
  make_top_impl(
    type_name,
    "schema",
    "FromExtractor",
    "from_extractor",
    ["e"],
    body,
  )
}

///|
fn build_variant_to_element_value(
  type_name : String,
  cases : Array[(String, VariantPayload)],
) -> @syntax.Impl {
  let match_cases : Array[@syntax.Case] = []
  for i, c in cases {
    let (name, payload) = c
    match payload {
      None =>
        match_cases.push(
          make_case(
            make_constr_pattern(name, []),
            make_apply(make_qualified_expr("schema", "make_variant_value"), [
              make_uint_expr(i),
              make_constr_no_args("None"),
            ]),
          ),
        )
      Single(_pt) =>
        match_cases.push(
          make_case(
            make_constr_pattern(name, [make_var_pattern("field0")]),
            make_apply(make_qualified_expr("schema", "make_variant_value"), [
              make_uint_expr(i),
              make_apply(make_constr_no_args("Some"), [
                make_apply(
                  make_qualified_expr("schema", "to_element_value_as"),
                  [make_ident_expr("field0")],
                ),
              ]),
            ]),
          ),
        )
      Multi(fields) => {
        let field_pats = fields.mapi(fn(j, _f) {
          make_var_pattern("field\{j}")
        })
        let field_value_exprs = fields.mapi(fn(j, _f) {
          make_apply(make_qualified_expr("schema", "to_element_value_as"), [
            make_ident_expr("field\{j}"),
          ])
        })
        match_cases.push(
          make_case(
            make_constr_pattern(name, field_pats),
            make_apply(make_qualified_expr("schema", "make_variant_value"), [
              make_uint_expr(i),
              make_apply(make_constr_no_args("Some"), [
                make_apply(
                  make_qualified_expr("schema", "make_record_value"),
                  [make_array(field_value_exprs)],
                ),
              ]),
            ]),
          ),
        )
      }
    }
  }
  let body = make_match(make_ident_expr("self"), match_cases)
  make_top_impl(
    type_name,
    "schema",
    "ToElementValue",
    "to_element_value",
    ["self"],
    body,
  )
}
