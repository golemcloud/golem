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
      "  let entries : Array[@common.AgentConfigDeclaration] = []\n",
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
    buf.write_string("impl @config.ConfigField for \{name} with load(path) {\n")
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

///|
fn pascal_to_snake(name : String) -> String {
  let buf = StringBuilder::new()
  let mut first = true
  for ch in name {
    if ch >= 'A' && ch <= 'Z' {
      if not(first) {
        buf.write_char('_')
      }
      buf.write_char(Char::from_int(ch.to_int() + 32))
    } else {
      buf.write_char(ch)
    }
    first = false
  }
  buf.to_string()
}

///|
/// Generates `*Override` structs and `::empty()` constructors for all config
/// types. Secret fields are omitted, nested config fields become their own
/// Override type, and leaf fields become `Option[T]`.
pub fn generate_config_override_types(
  config_types : Array[ConfigTypeInfo],
) -> @list.List[@syntax.Impl] {
  let config_type_names : Map[String, ConfigTypeInfo] = {}
  for ct in config_types {
    config_type_names[ct.name] = ct
  }
  let buf = StringBuilder::new()
  for ct in config_types {
    let override_fields : Array[(String, String)] = []
    for field in ct.fields {
      let (field_name, pt) = field
      if is_secret_type(pt) {
        continue
      }
      match pt {
        Simple(name) =>
          if config_type_names.get(name) is Some(_) {
            override_fields.push((field_name, name + "Override"))
          } else {
            override_fields.push((field_name, param_type_to_type_string(pt)))
          }
        _ => override_fields.push((field_name, param_type_to_type_string(pt)))
      }
    }
    if override_fields.is_empty() {
      continue
    }
    let override_name = ct.name + "Override"
    buf.write_string("///|\n")
    buf.write_string("pub(all) struct \{override_name} {\n")
    for field in override_fields {
      let (fname, ftype) = field
      buf.write_string("  \{fname} : \{ftype}?\n")
    }
    buf.write_string("}\n\n")
    buf.write_string("///|\n")
    buf.write_string("pub fn \{override_name}::empty() -> \{override_name} {\n")
    let field_inits = override_fields.map(fn(f) { "\{f.0}: None" })
    let fields_str = field_inits.join(", ")
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

///|
/// Generates walker functions that convert `*Override` structs into
/// `Array[@common.TypedAgentConfigValue]` for use with config override RPC
/// client variants.
pub fn generate_config_override_walkers(
  config_types : Array[ConfigTypeInfo],
) -> @list.List[@syntax.Impl] {
  let config_type_names : Map[String, ConfigTypeInfo] = {}
  for ct in config_types {
    config_type_names[ct.name] = ct
  }
  let buf = StringBuilder::new()
  for ct in config_types {
    let has_non_secret = ct.fields
      .iter()
      .any(fn(f) { not(is_secret_type(f.1)) })
    if not(has_non_secret) {
      continue
    }
    let override_name = ct.name + "Override"
    let fn_name = "collect_" + pascal_to_snake(ct.name) + "_overrides"
    buf.write_string("///|\n")
    buf.write_string(
      "pub fn \{fn_name}(config : \{override_name}, path : Array[String]) -> Array[@common.TypedAgentConfigValue] raise @common.AgentError {\n",
    )
    buf.write_string(
      "  let result : Array[@common.TypedAgentConfigValue] = []\n",
    )
    for field in ct.fields {
      let (field_name, pt) = field
      if is_secret_type(pt) {
        continue
      }
      let is_nested = match pt {
        Simple(name) => config_type_names.get(name) is Some(_)
        _ => false
      }
      buf.write_string("  match config.\{field_name} {\n")
      buf.write_string("    Some(v) => {\n")
      buf.write_string("      let p = path.copy()\n")
      buf.write_string("      p.push(\"\{field_name}\")\n")
      if is_nested {
        match pt {
          Simple(name) => {
            let nested_fn = "collect_" + pascal_to_snake(name) + "_overrides"
            buf.write_string("      result.append(\{nested_fn}!(v, p))\n")
          }
          _ => ()
        }
      } else {
        buf.write_string("      result.push(@rpc.typed_config_value!(p, v))\n")
      }
      buf.write_string("    }\n")
      buf.write_string("    None => ()\n")
      buf.write_string("  }\n")
    }
    buf.write_string("  result\n")
    buf.write_string("}\n\n")
  }
  let source = buf.to_string()
  if source.is_empty() {
    return @list.List::from_array([])
  }
  let (parsed, _) = @parser.parse_string(source)
  parsed
}
