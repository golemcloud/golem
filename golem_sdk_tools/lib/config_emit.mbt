///|
/// Generates `ConfigField` trait implementations (`collect_entries` and `load`)
/// for all parsed config types annotated with `#derive.config`.
pub fn generate_config_field_impls(
  config_types : Array[ConfigTypeInfo],
) -> @list.List[@syntax.Impl] {
  let buf = StringBuilder::new()
  for ct in config_types {
    let name = ct.name
    // Generate collect_entries impl
    buf.write_string("///|\n")
    buf.write_string(
      "impl @config.ConfigField for \{name} with collect_entries(path) {\n",
    )
    buf.write_string(
      "  let entries : Array[@common.ConfigKeyValueType] = []\n",
    )
    for i, field in ct.fields {
      let (field_name, pt) = field
      let type_str = param_type_to_type_string(pt)
      buf.write_string("  let fp\{i} = path.copy()\n")
      buf.write_string("  fp\{i}.push(\"\{field_name}\")\n")
      buf.write_string(
        "  entries.append(@config.collect_entries_of!((@schema.TypeTag::{} : @schema.TypeTag[\{type_str}]), fp\{i}))\n",
      )
    }
    buf.write_string("  entries\n")
    buf.write_string("}\n\n")
    // Generate load impl
    buf.write_string("///|\n")
    buf.write_string(
      "impl @config.ConfigField for \{name} with load(path) {\n",
    )
    for i, field in ct.fields {
      let (field_name, pt) = field
      let type_str = param_type_to_type_string(pt)
      buf.write_string("  let fp\{i} = path.copy()\n")
      buf.write_string("  fp\{i}.push(\"\{field_name}\")\n")
      buf.write_string(
        "  let \{field_name} : \{type_str} = @config.load_field!((@schema.TypeTag::{} : @schema.TypeTag[\{type_str}]), fp\{i})\n",
      )
    }
    // Build the struct literal with all field names
    let field_names = ct.fields.map(fn(f) { f.0 })
    let fields_str = field_names.join(", ")
    buf.write_string("  { \{fields_str} }\n")
    buf.write_string("}\n\n")
  }
  let source = buf.to_string()
  if source.is_empty() {
    return @list.List::from_array([])
  }
  let (parsed, _) = @parser.parse_string(source)
  parsed
}
