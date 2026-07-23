// Emission of `@multimodal.MultimodalModality` impls for `#derive.multimodal`
// enums, targeting the schema-native model (`golem:core/types@2.0.0`). A
// `#derive.multimodal` enum's variants are the modalities; the generated impl
// drives `@multimodal.Multimodal[T]`, which serializes to a `List(Variant(...))`
// whose list node carries `role = Multimodal`. There is no legacy
// `@types.Multimodal` / `MultimodalModality` carrier and no `DataValue` /
// `ElementValue`.
//
// Like the other emitters, code is built as a MoonBit-source string and parsed
// back to `@syntax.Impl` AST via `parse_generated_impls`, then formatted.

///|
/// Generates `@multimodal.MultimodalModality` impls for all parsed multimodal
/// enums. `type_id_prefix` (the package's dotted path) makes the modality
/// type-id package-qualified, exactly like value types.
pub fn generate_multimodal_impls(
  enums : Array[MultimodalEnumInfo],
  type_id_prefix? : String = "",
) -> @list.List[@syntax.Impl] {
  let buf = StringBuilder::new()
  for e in enums {
    let id_base = if type_id_prefix.is_empty() {
      e.name
    } else {
      type_id_prefix + "." + e.name
    }
    emit_multimodal_impls(buf, e.name, id_base, e.variants)
  }
  let source = buf.to_string()
  if source.is_empty() {
    return @list.List::from_array([])
  }
  parse_generated_impls(source, "multimodal enums")
}

///|
fn emit_multimodal_impls(
  buf : StringBuilder,
  name : String,
  id_base : String,
  variants : Array[(String, ParamType)],
) -> Unit {
  let id = escape_string_lit(id_base)
  let count = variants.length()
  // multimodal_type_id
  buf.write_string("///|\n")
  buf.write_string(
    "pub impl @multimodal.MultimodalModality for \{name} with multimodal_type_id() {\n",
  )
  buf.write_string("  \"\{id}\"\n}\n\n")
  // multimodal_cases
  buf.write_string("///|\n")
  buf.write_string(
    "pub impl @multimodal.MultimodalModality for \{name} with multimodal_cases(b) {\n",
  )
  buf.write_string("  [\n")
  for variant in variants {
    let (cname, pt) = variant
    let ts = param_type_to_type_string(pt)
    buf.write_string(
      "    @model.variant_case(\"\{escape_string_lit(cname)}\", payload=\{register_in_with_expr(ts)}),\n",
    )
  }
  buf.write_string("  ]\n}\n\n")
  // to_modality_value
  buf.write_string("///|\n")
  buf.write_string(
    "pub impl @multimodal.MultimodalModality for \{name} with to_modality_value(self) {\n",
  )
  buf.write_string("  match self {\n")
  for i, variant in variants {
    let (cname, _) = variant
    buf.write_string("    \{cname}(v) => (\{i}U, @schema.to_value_as(v))\n")
  }
  buf.write_string("  }\n}\n\n")
  // from_modality_value
  buf.write_string("///|\n")
  buf.write_string(
    "pub impl @multimodal.MultimodalModality for \{name} with from_modality_value(case, payload) {\n",
  )
  buf.write_string("  match case {\n")
  for i, variant in variants {
    let (cname, _) = variant
    buf.write_string("    \{i} => \{cname}(@schema.from_value_as(payload))\n")
  }
  buf.write_string("    other => {\n")
  buf.write_string(
    "      let err = @schema.FromSchemaError::out_of_range(other, \{count}U, \"\{id}\")\n",
  )
  buf.write_string("      raise err\n")
  buf.write_string("    }\n")
  buf.write_string("  }\n}\n\n")
}
