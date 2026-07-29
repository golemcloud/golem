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
) -> String {
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
        let suffix = lines[i].unsafe_substring(
          start=entry.code_end,
          end=lines[i].length(),
        )
        lines[i] = entry.indent + "\"\{import_path}\" \{import_alias}," + suffix
        return lines.join("\n")
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
