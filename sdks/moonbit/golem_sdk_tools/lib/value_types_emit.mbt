// Emission of `@schema.IntoSchema` + `@schema.FromSchema` impls for
// `#derive.golem_schema` value types (records, simple enums, variant enums),
// targeting the recursive schema model (`golem:core/types@2.0.0`).
//
// The generated code mirrors the hand-written, compiling golden in
// `golem_sdk/schema/records_test.mbt`, `@`-qualified for the user's package
// (`@schema` = `golem_sdk/schema`, `@model` = `golem_sdk/schema_model`).
//
// Like `config_emit.mbt`, code is built as a MoonBit-source string and parsed
// back into `@syntax.Impl` AST via `@parser.parse_string`, then handed to the
// formatter — far simpler and more maintainable than hand-building AST.

///|
/// Generates `IntoSchema` (`type_id`/`register_in`/`to_value`) and `FromSchema`
/// (`from_value`) impls for all parsed value types.
pub fn generate_value_type_impls(
  types : Array[ValueTypeInfo],
  type_id_prefix? : String = "",
) -> @list.List[@syntax.Impl] {
  let buf = StringBuilder::new()
  for t in types {
    // Default `type_id()` is the package-qualified dotted MoonBit type name
    // (the stable identity used for Refs / def dedup / agent-graph merge). The
    // prefix is the package path with `/` -> `.`; empty in unit tests.
    let id_base = if type_id_prefix.is_empty() {
      t.name
    } else {
      type_id_prefix + "." + t.name
    }
    match t.kind {
      Record(fields) => emit_record_impls(buf, t.name, id_base, fields)
      SimpleEnum(cases) => emit_simple_enum_impls(buf, t.name, id_base, cases)
      VariantEnum(cases) => emit_variant_impls(buf, t.name, id_base, cases)
    }
  }
  let source = buf.to_string()
  if source.is_empty() {
    return @list.List::from_array([])
  }
  parse_generated_impls(source, "value types")
}

///|
/// Escapes a MoonBit string literal body (backslash, quote, newline).
fn escape_string_lit(s : String) -> String {
  let buf = StringBuilder::new()
  for ch in s {
    match ch {
      '\\' => buf.write_string("\\\\")
      '"' => buf.write_string("\\\"")
      '\n' => buf.write_string("\\n")
      _ => buf.write_char(ch)
    }
  }
  buf.to_string()
}

///|
/// Emits a `@schema.register_in_with(b, (@schema.TypeTag::{} : @schema.TypeTag[T]))`
/// expression that registers (and refers to) the schema of type `T`.
fn register_in_with_expr(type_str : String) -> String {
  "@schema.register_in_with(b, (@schema.TypeTag::{  } : @schema.TypeTag[\{type_str}]))"
}

// ============================================================
// Records
// ============================================================

///|
fn emit_record_impls(
  buf : StringBuilder,
  name : String,
  id_base : String,
  fields : Array[(String, ParamType)],
) -> Unit {
  let id = escape_string_lit(id_base)
  // type_id
  buf.write_string("///|\n")
  buf.write_string("pub impl @schema.IntoSchema for \{name} with type_id() {\n")
  buf.write_string("  \"\{id}\"\n}\n\n")
  // register_in
  buf.write_string("///|\n")
  buf.write_string(
    "pub impl @schema.IntoSchema for \{name} with register_in(b) {\n",
  )
  buf.write_string("  b.register(\n    \"\{id}\",\n")
  buf.write_string("    () => @model.schema_type(Record([\n")
  for field in fields {
    let (fname, pt) = field
    let ts = param_type_to_type_string(pt)
    buf.write_string(
      "      @model.field(\"\{escape_string_lit(fname)}\", \{register_in_with_expr(ts)}),\n",
    )
  }
  buf.write_string("    ])),\n    Some(\"\{id}\"),\n  )\n}\n\n")
  // to_value
  buf.write_string("///|\n")
  buf.write_string(
    "pub impl @schema.IntoSchema for \{name} with to_value(self) {\n",
  )
  buf.write_string("  Record([\n")
  for field in fields {
    buf.write_string("    @schema.to_value_as(self.\{field.0}),\n")
  }
  buf.write_string("  ])\n}\n\n")
  // from_value
  buf.write_string("///|\n")
  buf.write_string(
    "pub impl @schema.FromSchema for \{name} with from_value(v) {\n",
  )
  buf.write_string("  let fields = @schema.expect_record(v, \"\{id}\")\n")
  for i, field in fields {
    let (fname, pt) = field
    let ts = param_type_to_type_string(pt)
    buf.write_string(
      "  let \{fname} : \{ts} = @schema.from_value_as(@schema.record_field(fields, \{i}, \"\{id}\"))\n",
    )
  }
  let field_names = fields.map(fn(f) { f.0 }).join(", ")
  buf.write_string("  { \{field_names} }\n}\n\n")
}

// ============================================================
// Simple enums
// ============================================================

///|
fn emit_simple_enum_impls(
  buf : StringBuilder,
  name : String,
  id_base : String,
  cases : Array[String],
) -> Unit {
  let id = escape_string_lit(id_base)
  let count = cases.length()
  // type_id
  buf.write_string("///|\n")
  buf.write_string("pub impl @schema.IntoSchema for \{name} with type_id() {\n")
  buf.write_string("  \"\{id}\"\n}\n\n")
  // register_in
  let case_lits = cases.map(fn(c) { "\"\{escape_string_lit(c)}\"" }).join(", ")
  buf.write_string("///|\n")
  buf.write_string(
    "pub impl @schema.IntoSchema for \{name} with register_in(b) {\n",
  )
  buf.write_string(
    "  b.register(\"\{id}\", () => @model.schema_type(Enum([\{case_lits}])), Some(\"\{id}\"))\n}\n\n",
  )
  // to_value
  buf.write_string("///|\n")
  buf.write_string(
    "pub impl @schema.IntoSchema for \{name} with to_value(self) {\n",
  )
  buf.write_string("  match self {\n")
  for i, c in cases {
    buf.write_string("    \{c} => Enum(\{i})\n")
  }
  buf.write_string("  }\n}\n\n")
  // from_value
  buf.write_string("///|\n")
  buf.write_string(
    "pub impl @schema.FromSchema for \{name} with from_value(v) {\n",
  )
  buf.write_string("  let case = @schema.expect_enum(v, \"\{id}\")\n")
  buf.write_string("  @schema.check_case_index(case, \{count}, \"\{id}\")\n")
  buf.write_string("  match case {\n")
  for i, c in cases {
    buf.write_string("    \{i} => \{c}\n")
  }
  emit_unreachable_arm(buf, "unreachable enum case for \{id}")
  buf.write_string("  }\n}\n\n")
}

///|
/// Emits the unreachable `_` match arm for a `from_value` decoder. Bind-then-
/// raise (rather than `_ => raise <long-expr>`) keeps the formatter from
/// breaking `raise` onto its own line, which the MoonBit parser rejects.
fn emit_unreachable_arm(buf : StringBuilder, message : String) -> Unit {
  buf.write_string("    _ => {\n")
  buf.write_string(
    "      let err = @schema.FromSchemaError::custom(\"\{message}\")\n",
  )
  buf.write_string("      raise err\n")
  buf.write_string("    }\n")
}

// ============================================================
// Variant enums
// ============================================================

///|
/// The payload schema type-string for a variant case, or `None` for a unit
/// case. A multi-field payload (labelled or positional) is encoded as a tuple.
fn variant_payload_type_string(payload : VariantPayload) -> String? {
  match payload {
    None => Option::None
    Single(pt) => Some(param_type_to_type_string(pt))
    Multi(fields, _) => {
      let parts = fields.map(fn(f) { param_type_to_type_string(f.1) })
      Some("(" + parts.join(", ") + ")")
    }
  }
}

///|
fn emit_variant_impls(
  buf : StringBuilder,
  name : String,
  id_base : String,
  cases : Array[(String, VariantPayload)],
) -> Unit {
  let id = escape_string_lit(id_base)
  let count = cases.length()
  // type_id
  buf.write_string("///|\n")
  buf.write_string("pub impl @schema.IntoSchema for \{name} with type_id() {\n")
  buf.write_string("  \"\{id}\"\n}\n\n")
  // register_in
  buf.write_string("///|\n")
  buf.write_string(
    "pub impl @schema.IntoSchema for \{name} with register_in(b) {\n",
  )
  buf.write_string("  b.register(\n    \"\{id}\",\n")
  buf.write_string("    () => @model.schema_type(Variant([\n")
  for case in cases {
    let (cname, payload) = case
    let cl = escape_string_lit(cname)
    match variant_payload_type_string(payload) {
      None => buf.write_string("      @model.variant_case(\"\{cl}\"),\n")
      Some(ts) =>
        buf.write_string(
          "      @model.variant_case(\"\{cl}\", payload=\{register_in_with_expr(ts)}),\n",
        )
    }
  }
  buf.write_string("    ])),\n    Some(\"\{id}\"),\n  )\n}\n\n")
  // to_value
  buf.write_string("///|\n")
  buf.write_string(
    "pub impl @schema.IntoSchema for \{name} with to_value(self) {\n",
  )
  buf.write_string("  match self {\n")
  for i, case in cases {
    let (cname, payload) = case
    match payload {
      None => buf.write_string("    \{cname} => Variant(\{i}, None)\n")
      Single(_) =>
        buf.write_string(
          "    \{cname}(field0) => Variant(\{i}, Some(@schema.to_value_as(field0)))\n",
        )
      Multi(fields, labelled) => {
        let (pat, tuple) = variant_match_binding(cname, fields, labelled)
        buf.write_string(
          "    \{pat} => Variant(\{i}, Some(@schema.to_value_as(\{tuple})))\n",
        )
      }
    }
  }
  buf.write_string("  }\n}\n\n")
  // from_value
  buf.write_string("///|\n")
  buf.write_string(
    "pub impl @schema.FromSchema for \{name} with from_value(v) {\n",
  )
  buf.write_string(
    "  let (case, payload) = @schema.expect_variant(v, \"\{id}\")\n",
  )
  buf.write_string("  @schema.check_case_index(case, \{count}, \"\{id}\")\n")
  buf.write_string("  match case {\n")
  for i, case in cases {
    let (cname, payload) = case
    let cl = escape_string_lit(cname)
    match payload {
      None => {
        buf.write_string("    \{i} => {\n")
        buf.write_string(
          "      @schema.expect_no_variant_payload(payload, \"\{id}.\{cl}\")\n",
        )
        buf.write_string("      \{cname}\n    }\n")
      }
      Single(pt) => {
        let ts = param_type_to_type_string(pt)
        buf.write_string("    \{i} => {\n")
        buf.write_string(
          "      let p : \{ts} = @schema.from_value_as(@schema.expect_variant_payload(payload, \"\{id}.\{cl}\"))\n",
        )
        buf.write_string("      \{cname}(p)\n    }\n")
      }
      Multi(fields, labelled) => {
        let parts = fields.map(fn(f) { param_type_to_type_string(f.1) })
        let tuple_ty = "(" + parts.join(", ") + ")"
        let locals = fields.map(fn(f) { f.0 }).join(", ")
        let ctor = variant_ctor_call(cname, fields, labelled)
        buf.write_string("    \{i} => {\n")
        buf.write_string(
          "      let (\{locals}) : \{tuple_ty} = @schema.from_value_as(@schema.expect_variant_payload(payload, \"\{id}.\{cl}\"))\n",
        )
        buf.write_string("      \{ctor}\n    }\n")
      }
    }
  }
  emit_unreachable_arm(buf, "unreachable variant case for \{id}")
  buf.write_string("  }\n}\n\n")
}

///|
/// Builds the match pattern + tuple expression for a multi-field variant case in
/// `to_value`. Labelled fields use punned patterns (`X(a~, b~)`); positional
/// fields bind by name (`X(field0, field1)`). Both produce the same tuple value.
fn variant_match_binding(
  cname : String,
  fields : Array[(String, ParamType)],
  labelled : Bool,
) -> (String, String) {
  let names = fields.map(fn(f) { f.0 })
  let tuple = "(" + names.join(", ") + ")"
  let pat = if labelled {
    let bindings = names.map(fn(n) { "\{n}~" }).join(", ")
    "\{cname}(\{bindings})"
  } else {
    "\{cname}(" + names.join(", ") + ")"
  }
  (pat, tuple)
}

///|
/// Builds the constructor call for a multi-field variant case in `from_value`.
fn variant_ctor_call(
  cname : String,
  fields : Array[(String, ParamType)],
  labelled : Bool,
) -> String {
  let names = fields.map(fn(f) { f.0 })
  if labelled {
    let bindings = names.map(fn(n) { "\{n}~" }).join(", ")
    "\{cname}(\{bindings})"
  } else {
    "\{cname}(" + names.join(", ") + ")"
  }
}
