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
/// Updates a `moon.pkg` file's content to include or replace the `link: { ... }` block.
/// Preserves all other content (imports, is-main, pre-build, etc.).
pub fn ensure_pkg_import(
  content : String,
  import_path : String,
  import_alias : String,
) -> String {
  let lines : Array[String] = content
    .split("\n")
    .map(fn(sv) { sv.to_string() })
    .collect()
  for line in lines {
    let trimmed = line.trim()
    if trimmed.contains(import_path) {
      return content
    }
  }
  let mut import_start = -1
  let mut import_end = -1
  for i = 0; i < lines.length(); i = i + 1 {
    let trimmed = lines[i].trim()
    if trimmed.has_prefix("import") && trimmed.contains("{") {
      import_start = i
    }
    if import_start >= 0 && trimmed == "}" {
      import_end = i
      break
    }
  }
  let result : Array[String] = []
  if import_start >= 0 && import_end >= 0 {
    for i = 0; i < import_end; i = i + 1 {
      result.push(lines[i])
    }
    result.push("  \"\{import_path}\" \{import_alias},")
    for i = import_end; i < lines.length(); i = i + 1 {
      result.push(lines[i])
    }
  } else {
    result.push("import {")
    result.push("  \"\{import_path}\" \{import_alias},")
    result.push("}")
    result.push("")
    for line in lines {
      result.push(line)
    }
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
