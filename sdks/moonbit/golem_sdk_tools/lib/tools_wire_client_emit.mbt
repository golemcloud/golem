///|
fn emit_wire_client_error_decoder(command : ToolCommandInfo) -> String {
  if command.errors.is_empty() {
    return "fn(_, _) { None }"
  }
  let cases = command.errors.map(error => {
    let payload_type = error.payload.map(emit_type).unwrap_or("Unit")
    let value = match (error.payload, error.payload_label) {
      (Some(_), Some(label)) => error.name + "(" + label + "=__golem_payload)"
      (Some(_), None) => error.name + "(__golem_payload)"
      (None, _) => error.name
    }
    "\"" +
    escape_string_lit(moonbit_name_to_kebab_case(error.name)) +
    "\" => { let __golem_payload : " +
    payload_type +
    " = @schema.from_wire(__golem_value.value) catch { error => return Some(Err(repr(error))) }; Some(Ok(" +
    value +
    ")) }"
  })
  "fn(__golem_name, __golem_value) { match __golem_name { " +
  cases.join("\n") +
  "\n_ => None } }"
}

///|
fn emit_client_wire_input(
  buf : StringBuilder,
  fields : Array[ProjectedToolArg],
  captured : Array[ProjectedToolArg],
  has_prefix : Bool,
  asynchronous : Bool,
  stdin : String?,
) -> Unit {
  let schemas = fields.map(arg => {
    let schema = wire_client_field_type(arg, arg.type_.unwrap())
    "{ name: \"" +
    escape_string_lit(arg.canonical) +
    "\", body: " +
    schema +
    ", metadata: @schema.empty_wire_metadata() }"
  })
  buf.write_string(
    "  let __golem_values : Array[&@schema.IntoWire] = " +
    (if has_prefix { "self.prefix.copy()" } else { "[]" }) +
    "\n",
  )
  for arg in fields[captured.length():] {
    buf.write_string("  __golem_values.push(" + arg.source + ")\n")
  }
  let encoder = if asynchronous {
    "@schema.into_wire_record_async"
  } else {
    "@schema.into_wire_record"
  }
  let cleanup = stdin.map(source => source + ".drop_sync(); ").unwrap_or("")
  buf.write_string(
    "  let __golem_schema = @schema.WireTypeEncoder::new()\n" +
    "  let __golem_root = __golem_schema.push(@types.RecordType([" +
    schemas.join(", ") +
    "])) catch { error => { @schema.release_wire_fields(__golem_values); " +
    cleanup +
    "return Err(@tool.tool_protocol_error(repr(error))) } }\n",
  )
  let cancel = if asynchronous {
    "@asyncCore.Cancelled::Cancelled as error => { " +
    cleanup +
    "raise error }\n"
  } else {
    ""
  }
  buf.write_string(
    "  let __golem_value = " +
    encoder +
    "(__golem_values) catch { " +
    cancel +
    "error => { " +
    cleanup +
    "return Err(@tool.tool_protocol_error(repr(error))) } }\n",
  )
  for index, field in fields {
    if field.info
      .map(info => {
        info.regex is Some(_) ||
        info.min_length is Some(_) ||
        info.max_length is Some(_)
      })
      .unwrap_or(false) {
      buf.write_string(
        "  @tool.wire_text_input_field(__golem_value, " +
        index.to_string() +
        ")\n",
      )
    }
  }
  buf.write_string(
    "  let __golem_input : @types.TypedSchemaValue = { graph: { type_nodes: __golem_schema.nodes, defs: __golem_schema.defs, root: __golem_root }, value: __golem_value }\n",
  )
}

///|
fn wire_client_field_type(arg : ProjectedToolArg, type_ : ParamType) -> String {
  match type_ {
    Optional(inner) =>
      "__golem_schema.push(@types.OptionType(" +
      wire_client_field_type(arg, inner) +
      "))"
    List(inner) => {
      let element = if arg.placement is Tail {
        {
          ..arg,
          info: arg.info.map(info => { ..info, min: None, max: None, }),
        }
      } else {
        arg
      }
      "__golem_schema.push(@types.ListType(" +
      wire_client_field_type(element, inner) +
      "))"
    }
    _ =>
      "(" +
      refined_graph({ ..arg, value_type: Some(type_), }, wire=true) +
      ").index"
  }
}
