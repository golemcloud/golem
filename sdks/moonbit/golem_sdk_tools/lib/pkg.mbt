///|
/// Parses the `exports` array from a `moon.pkg` file's `link.wasm.exports` section.
/// Returns the export entries as strings (e.g. "wasmExportInitialize:golem:agent/guest#initialize").
pub fn parse_pkg_exports(content : String) -> Array[String] {
  let exports : Array[String] = []
  let lines : Array[String] = content
    .split("\n")
    .map(fn(sv) { sv.to_string() })
    .collect()
  let mut in_exports = false
  for i = 0; i < lines.length(); i = i + 1 {
    let trimmed = lines[i].trim()
    if in_exports {
      if trimmed.has_prefix("]") {
        break
      }
      let line = lines[i]
      match line.find("\"") {
        Some(first_q) => {
          let after = line.unsafe_substring(
            start=first_q + 1,
            end=line.length(),
          )
          match after.rev_find("\"") {
            Some(last_q) =>
              exports.push(after.unsafe_substring(start=0, end=last_q))
            None => ()
          }
        }
        None => ()
      }
    } else if trimmed.contains("\"exports\"") {
      in_exports = true
    }
  }
  exports
}

///|
/// Transforms export entries from SDK-internal names to consumer names.
/// Applies the same `mbt_ffi_` prefix stripping as the reexport generator.
pub fn transform_exports(exports : Array[String]) -> Array[String] {
  exports.map(fn(entry) {
    match entry.find(":") {
      Some(colon) => {
        let fn_name = entry.unsafe_substring(start=0, end=colon)
        let wit_name = entry.unsafe_substring(
          start=colon + 1,
          end=entry.length(),
        )
        let user_name = match fn_name.strip_prefix("mbt_ffi_") {
          Some(stripped) => stripped.to_string()
          None => fn_name
        }
        "\{user_name}:\{wit_name}"
      }
      None => entry
    }
  })
}

///|
/// Generates the `link: { ... },` block lines for a `moon.pkg` file.
fn generate_link_block_lines(exports : Array[String]) -> Array[String] {
  let lines : Array[String] = []
  lines.push("  link: {")
  lines.push("    \"wasm\": {")
  lines.push("      \"export-memory-name\": \"memory\",")
  lines.push("      \"exports\": [")
  for i, entry in exports {
    let comma = if i < exports.length() - 1 { "," } else { "," }
    lines.push("        \"\{entry}\"\{comma}")
  }
  lines.push("      ],")
  lines.push("      \"heap-start-address\": 16,")
  lines.push("    },")
  lines.push("  },")
  lines
}

///|
priv struct PkgImportEntry {
  path : String
  effective_alias : String
  indent : String
  code_end : Int
  has_comma : Bool
}

///|
fn pkg_line_code(line : String) -> String {
  let code_end = match line.find("//") {
    Some(index) => index
    None => line.length()
  }
  line.unsafe_substring(start=0, end=code_end).trim().to_owned()
}

///|
fn find_regular_import_block(lines : Array[String]) -> (Int, Int)? {
  let mut import_start = -1
  for i, line in lines {
    let code = pkg_line_code(line)
    if import_start < 0 {
      if code == "import {" {
        import_start = i
      }
    } else if code.has_prefix("}") {
      if code == "}" {
        return Some((import_start, i))
      }
      import_start = -1
    }
  }
  None
}

///|
fn pkg_default_alias(path : String) -> String {
  match path.rev_find("/") {
    Some(index) =>
      "@" + path.unsafe_substring(start=index + 1, end=path.length())
    None => "@" + path
  }
}

///|
fn parse_pkg_import_entry(line : String) -> PkgImportEntry? {
  let comment_start = match line.find("//") {
    Some(index) => index
    None => line.length()
  }
  let uncommented = line.unsafe_substring(start=0, end=comment_start)
  let code_end = uncommented.trim_end().length()
  let code = uncommented.trim().to_owned()
  guard code.has_prefix("\"") else { return None }
  let after_open = code.unsafe_substring(start=1, end=code.length())
  let quote = match after_open.find("\"") {
    Some(index) => index
    None => return None
  }
  let path = after_open.unsafe_substring(start=0, end=quote)
  let rest = after_open
    .unsafe_substring(start=quote + 1, end=after_open.length())
    .trim()
    .to_owned()
  let rest_without_comma = match rest.strip_suffix(",") {
    Some(value) => value.trim().to_owned()
    None => rest
  }
  let effective_alias = match rest_without_comma.rev_find("@") {
    Some(index) =>
      rest_without_comma
      .unsafe_substring(start=index, end=rest_without_comma.length())
      .trim()
      .to_owned()
    None => pkg_default_alias(path)
  }
  let indent_length = line.length() - line.trim_start().length()
  Some({
    path,
    effective_alias,
    indent: line.unsafe_substring(start=0, end=indent_length),
    code_end,
    has_comma: code.has_suffix(","),
  })
}

///|
/// Ensures a valid, formatted `moon.pkg` has the requested package in its
/// regular import block with the requested effective alias. Test-only imports
/// and unrelated package content are left unchanged.
pub fn ensure_pkg_import(
  content : String,
  import_path : String,
  import_alias : String,
) -> String raise ComponentRoleError {
  let lines : Array[String] = content
    .split("\n")
    .map(fn(sv) { sv.to_string() })
    .collect()
  let block = find_regular_import_block(lines)
  guard block is Some((import_start, import_end)) else {
    let result : Array[String] = [
      "import {",
      "  \"\{import_path}\" \{import_alias},",
      "}",
      "",
    ]
    for line in lines {
      result.push(line)
    }
    return result.join("\n")
  }
  let mut first_indent : String? = None
  let mut last_entry : (Int, PkgImportEntry)? = None
  for i in (import_start + 1)..<import_end {
    if parse_pkg_import_entry(lines[i]) is Some(entry) {
      if first_indent is None {
        first_indent = Some(entry.indent)
      }
      last_entry = Some((i, entry))
      if entry.path == import_path {
        if entry.effective_alias == import_alias {
          return content
        }
        component_role_fail(
          "moon.pkg package '" +
          import_path +
          "' is already imported as '" +
          entry.effective_alias +
          "'; generated source requires alias '" +
          import_alias +
          "'",
        )
        abort("")
      }
      if entry.effective_alias == import_alias {
        component_role_fail(
          "moon.pkg import alias " +
          import_alias +
          " is already used by unrelated package '" +
          entry.path +
          "'",
        )
        abort("")
      }
    }
  }
  if last_entry is Some((index, entry)) && !entry.has_comma {
    let suffix = lines[index].unsafe_substring(
      start=entry.code_end,
      end=lines[index].length(),
    )
    lines[index] = lines[index].unsafe_substring(start=0, end=entry.code_end) +
      "," +
      suffix
  }
  let indent = match first_indent {
    Some(indent) => indent
    None => "  "
  }
  let result : Array[String] = []
  for i in 0..<import_end {
    result.push(lines[i])
  }
  result.push(indent + "\"\{import_path}\" \{import_alias},")
  for i in import_end..<lines.length() {
    result.push(lines[i])
  }
  result.join("\n")
}

///|
fn is_generated_role_package(path : String) -> Bool {
  path == "golemcloud/golem_sdk/gen" ||
  path == "golemcloud/golem_sdk/gen-tool-middleware" ||
  path == "golemcloud/golem_sdk/gen-agent-tool-middleware"
}

///|
fn rewrite_pkg_import_entry(
  lines : Array[String],
  index : Int,
  entry : PkgImportEntry,
  path : String,
  import_alias : String,
) -> Unit {
  let suffix = lines[index].unsafe_substring(
    start=entry.code_end,
    end=lines[index].length(),
  )
  lines[index] = entry.indent + "\"\{path}\" \{import_alias}," + suffix
}

///|
/// Selects exactly one role-specific generated package under the stable @gen
/// alias. A previous role import is replaced, while unrelated ownership of the
/// alias is rejected.
pub fn ensure_role_gen_import(
  content : String,
  import_path : String,
) -> String raise ComponentRoleError {
  if not(is_generated_role_package(import_path)) {
    component_role_fail(
      "Unsupported generated role package '" + import_path + "'",
    )
  }
  let lines : Array[String] = content
    .split("\n")
    .map(fn(sv) { sv.to_string() })
    .collect()
  guard find_regular_import_block(lines) is Some((import_start, import_end)) else {
    return ensure_pkg_import(content, import_path, "@gen")
  }
  let mut gen_alias_index = -1
  let mut gen_alias_entry : PkgImportEntry? = None
  let mut selected_index = -1
  let mut selected_entry : PkgImportEntry? = None
  for i in (import_start + 1)..<import_end {
    if parse_pkg_import_entry(lines[i]) is Some(entry) {
      if entry.path == import_path {
        if selected_index >= 0 {
          component_role_fail(
            "Generated role package '" +
            import_path +
            "' is imported more than once",
          )
        }
        selected_index = i
        selected_entry = Some(entry)
      }
      if entry.effective_alias == "@gen" {
        if gen_alias_index >= 0 {
          component_role_fail(
            "moon.pkg import alias @gen is used more than once",
          )
        }
        if not(is_generated_role_package(entry.path)) {
          component_role_fail(
            "moon.pkg import alias @gen is already used by unrelated package '" +
            entry.path +
            "'",
          )
        }
        gen_alias_index = i
        gen_alias_entry = Some(entry)
      }
    }
  }
  if selected_index >= 0 && selected_entry is Some(entry) {
    rewrite_pkg_import_entry(lines, selected_index, entry, import_path, "@gen")
    if gen_alias_index >= 0 && gen_alias_index != selected_index {
      let result : Array[String] = []
      for i, line in lines {
        if i != gen_alias_index {
          result.push(line)
        }
      }
      return result.join("\n")
    }
    return lines.join("\n")
  }
  if gen_alias_index >= 0 && gen_alias_entry is Some(entry) {
    rewrite_pkg_import_entry(lines, gen_alias_index, entry, import_path, "@gen")
    return lines.join("\n")
  }
  ensure_pkg_import(content, import_path, "@gen")
}

///|
fn is_role_tool_package(path : String) -> Bool {
  path == "golemcloud/golem_sdk/tool" ||
  path == "golemcloud/golem_sdk/tool-core"
}

///|
/// Selects the host-neutral or ambient tool package under the stable @tool
/// alias without rewriting user-owned imports.
pub fn ensure_role_tool_import(
  content : String,
  import_path : String,
) -> String raise ComponentRoleError {
  if !is_role_tool_package(import_path) {
    component_role_fail("Unsupported role tool package '" + import_path + "'")
  }
  let lines : Array[String] = content
    .split("\n")
    .map(fn(sv) { sv.to_string() })
    .collect()
  if find_regular_import_block(lines) is Some((import_start, import_end)) {
    for i in (import_start + 1)..<import_end {
      if parse_pkg_import_entry(lines[i]) is Some(entry) &&
        is_role_tool_package(entry.path) &&
        entry.path != import_path {
        component_role_fail(
          "moon.pkg role tool package '" +
          entry.path +
          "' conflicts with required package '" +
          import_path +
          "'",
        )
        abort("")
      }
    }
  }
  ensure_pkg_import(content, import_path, "@tool")
}

///|
let generated_import_marker = "// Generated by golem_sdk_tools"

///|
fn is_generated_import_entry(line : String) -> Bool {
  line.trim().has_suffix(generated_import_marker) &&
  parse_pkg_import_entry(line) is Some(_)
}

///|
fn remove_owned_generated_imports(content : String) -> String {
  let lines : Array[String] = content
    .split("\n")
    .map(fn(sv) { sv.to_string() })
    .collect()
  guard find_regular_import_block(lines) is Some((import_start, import_end)) else {
    return content
  }
  let result : Array[String] = []
  for i, line in lines {
    if i > import_start && i < import_end && is_generated_import_entry(line) {
      continue
    }
    result.push(line)
  }
  result.join("\n")
}

///|
fn validate_user_import_for_requirement(
  content : String,
  import_path : String,
  import_alias : String,
) -> Bool raise ComponentRoleError {
  let lines : Array[String] = content
    .split("\n")
    .map(fn(sv) { sv.to_string() })
    .collect()
  guard find_regular_import_block(lines) is Some((import_start, import_end)) else {
    return false
  }
  let mut matching_path_count = 0
  for i in (import_start + 1)..<import_end {
    if parse_pkg_import_entry(lines[i]) is Some(entry) {
      if entry.path == import_path {
        matching_path_count += 1
        if entry.effective_alias != import_alias {
          component_role_fail(
            "moon.pkg package '" +
            import_path +
            "' is already imported as '" +
            entry.effective_alias +
            "'; generated source requires alias '" +
            import_alias +
            "'",
          )
          abort("")
        }
      }
      if entry.path != import_path && entry.effective_alias == import_alias {
        component_role_fail(
          "moon.pkg import alias " +
          import_alias +
          " is already used by unrelated package '" +
          entry.path +
          "'",
        )
        abort("")
      }
    }
  }
  if matching_path_count > 1 {
    component_role_fail(
      "moon.pkg package '" + import_path + "' is imported more than once",
    )
    abort("")
  }
  matching_path_count == 1
}

///|
pub fn reconcile_pure_role_pkg_imports(
  content : String,
) -> String raise ComponentRoleError {
  let lines : Array[String] = content
    .split("\n")
    .map(fn(sv) { sv.to_string() })
    .collect()
  guard find_regular_import_block(lines) is Some((import_start, import_end)) else {
    return content
  }
  let result : Array[String] = []
  let mut changed = false
  for i, line in lines {
    if i > import_start &&
      i < import_end &&
      parse_pkg_import_entry(line) is Some(entry) &&
      entry.path == "golemcloud/golem_sdk/tool" {
      if is_generated_import_entry(line) {
        changed = true
        continue
      } else {
        component_role_fail(
          "Pure tool-middleware moon.pkg must not import ambient package 'golemcloud/golem_sdk/tool'",
        )
        return content
      }
    }
    result.push(line)
  }
  if changed {
    result.join("\n")
  } else {
    content
  }
}

///|
fn mark_generated_import(content : String, path : String) -> String {
  let lines : Array[String] = content
    .split("\n")
    .map(fn(sv) { sv.to_string() })
    .collect()
  guard find_regular_import_block(lines) is Some((import_start, import_end)) else {
    return content
  }
  for i in (import_start + 1)..<import_end {
    if parse_pkg_import_entry(lines[i]) is Some(entry) && entry.path == path {
      if !is_generated_import_entry(lines[i]) {
        lines[i] = lines[i] + " " + generated_import_marker
      }
      return lines.join("\n")
    }
  }
  content
}

///|
/// Reconciles imports owned by generated source. Existing user-owned imports
/// are reused but never marked for later removal. Imports inserted by this
/// function are tagged and removed once no generated file requires them.
pub fn reconcile_generated_pkg_imports(
  content : String,
  required : Array[(String, String)],
  forbid_ambient_tool? : Bool = false,
) -> String raise ComponentRoleError {
  let mut result = remove_owned_generated_imports(content)
  if forbid_ambient_tool {
    result = reconcile_pure_role_pkg_imports(result)
  }
  let added : Map[String, Unit] = {}
  for item in required {
    let (path, import_alias) = item
    if added.contains(path) {
      continue
    }
    added[path] = ()
    let reused = validate_user_import_for_requirement(
      result, path, import_alias,
    )
    if !reused {
      result = ensure_pkg_import(result, path, import_alias)
      result = mark_generated_import(result, path)
    }
  }
  result
}

///|
/// Updates a `moon.pkg` file's content to include or replace the `link: { ... }` block.
/// Preserves all other content (imports, is-main, pre-build, etc.).
pub fn update_pkg_link(content : String, exports : Array[String]) -> String {
  let link_lines = generate_link_block_lines(exports)
  let lines : Array[String] = content
    .split("\n")
    .map(fn(sv) { sv.to_string() })
    .collect()
  // Find existing link: { line
  let mut link_start = -1
  for i = 0; i < lines.length(); i = i + 1 {
    let trimmed = lines[i].trim()
    if trimmed.has_prefix("link:") || trimmed.has_prefix("link :") {
      link_start = i
      break
    }
  }
  let result : Array[String] = []
  if link_start >= 0 {
    // Replace existing link block
    let mut depth = 0
    let mut link_end = link_start
    for i = link_start; i < lines.length(); i = i + 1 {
      for c in lines[i] {
        if c == '{' {
          depth = depth + 1
        }
        if c == '}' {
          depth = depth - 1
        }
      }
      if depth == 0 {
        link_end = i
        break
      }
    }
    for i = 0; i < link_start; i = i + 1 {
      result.push(lines[i])
    }
    for line in link_lines {
      result.push(line)
    }
    for i = link_end + 1; i < lines.length(); i = i + 1 {
      result.push(lines[i])
    }
  } else {
    // No link block — find options( and insert before its closing )
    let mut options_start = -1
    for i = 0; i < lines.length(); i = i + 1 {
      if lines[i].trim().has_prefix("options(") {
        options_start = i
        break
      }
    }
    if options_start >= 0 {
      let mut depth = 0
      let mut options_close = options_start
      for i = options_start; i < lines.length(); i = i + 1 {
        for c in lines[i] {
          if c == '(' {
            depth = depth + 1
          }
          if c == ')' {
            depth = depth - 1
          }
        }
        if depth == 0 {
          options_close = i
          break
        }
      }
      for i = 0; i < options_close; i = i + 1 {
        result.push(lines[i])
      }
      for line in link_lines {
        result.push(line)
      }
      for i = options_close; i < lines.length(); i = i + 1 {
        result.push(lines[i])
      }
    } else {
      // No options( — append options block
      for line in lines {
        result.push(line)
      }
      result.push("")
      result.push("options(")
      for line in link_lines {
        result.push(line)
      }
      result.push(")")
    }
  }
  result.join("\n")
}
