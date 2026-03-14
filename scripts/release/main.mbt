///|
let example_dir = "../golem_sdk_example1"

///|
let mod_json_path = "\{example_dir}/moon.mod.json"

///|
let golem_yaml_path = "\{example_dir}/golem.yaml"

///|
let sdk_mooncakes_path = ".mooncakes/golemcloud/golem_sdk"

///|
let tools_mooncakes_path = ".mooncakes/golemcloud/golem_sdk_tools"

///|
let dev_tools_path = "../golem_sdk_tools"

///|
let dev_sdk_path = "../golem_sdk"

///|
fn main {
  let args = @env.args()
  if args.length() < 2 {
    print_usage()
    return
  }
  if args[1] == "--dev" {
    revert_to_dev() catch {
      e => println("Error: \{e}")
    }
  } else {
    let sdk_version = args[1]
    let tools_version = if args.length() >= 3 { args[2] } else { sdk_version }
    prepare_release(sdk_version, tools_version) catch {
      e => println("Error: \{e}")
    }
  }
}

///|
fn print_usage() -> Unit {
  let usage =
    #|Usage: moon run -C scripts release -- <sdk-version> [tools-version]
    #|       moon run -C scripts release -- --dev
    #|
    #|Examples:
    #|  moon run -C scripts release -- 0.1.0            (same version for SDK and tools)
    #|  moon run -C scripts release -- 0.1.0 0.2.0      (different versions)
    #|  moon run -C scripts release -- --dev             (revert to development mode)
  println(usage)
}

///|
fn prepare_release(sdk_version : String, tools_version : String) -> Unit raise {
  println(
    "==> Preparing template release with golem_sdk \{sdk_version}, golem_sdk_tools \{tools_version}...",
  )
  // 1. moon.mod.json — compute new content
  let mod_json = @fs.read_file_to_string(mod_json_path)
  let mut m = replace_sdk_dep(
    mod_json,
    "\"golemcloud/golem_sdk\": \"\{sdk_version}\"",
  )
  m = remove_bin_deps(m)
  m = insert_bin_deps(m, "\"golemcloud/golem_sdk_tools\": \"\{tools_version}\"")
  // 2. golem.yaml — swap dev paths to mooncakes paths
  let yaml = @fs.read_file_to_string(golem_yaml_path)
  let mut y = yaml.replace_all(
    old="-C \{dev_tools_path}",
    new="-C \{tools_mooncakes_path}",
  )
  y = y.replace_all(old=" \{dev_sdk_path} ", new=" \{sdk_mooncakes_path} ")
  // Write all files only after all transforms succeed
  @fs.write_string_to_file(mod_json_path, m)
  @fs.write_string_to_file(golem_yaml_path, y)
  println("")
  println("Done. Files modified:")
  println(
    "  \{mod_json_path}  — golem_sdk dep: \{sdk_version}, golem_sdk_tools bin-dep: \{tools_version}",
  )
  println(
    "  \{golem_yaml_path} — build commands use mooncakes-installed tools",
  )
  println("")
  println("Review the changes, then copy \{example_dir}/ as your template.")
  println("To revert: moon run -C scripts release -- --dev")
}

///|
fn revert_to_dev() -> Unit raise {
  println("==> Reverting to development mode...")
  let path_dep = "\"golemcloud/golem_sdk\": {\n      \"path\": \"../golem_sdk\"\n    }"
  // 1. moon.mod.json — compute new content
  let mod_json = @fs.read_file_to_string(mod_json_path)
  let mut m = replace_sdk_dep(mod_json, path_dep)
  m = remove_bin_deps(m)
  // 2. golem.yaml — swap mooncakes paths back to dev paths
  let yaml = @fs.read_file_to_string(golem_yaml_path)
  let mut y = yaml.replace_all(
    old="-C \{tools_mooncakes_path}",
    new="-C \{dev_tools_path}",
  )
  y = y.replace_all(old=" \{sdk_mooncakes_path} ", new=" \{dev_sdk_path} ")
  // Write all files only after all transforms succeed
  @fs.write_string_to_file(mod_json_path, m)
  @fs.write_string_to_file(golem_yaml_path, y)
  println("Done. Now in development mode (local path deps).")
}

// --- String helpers ---

///|
fn split_lines(s : String) -> Array[String] {
  s.split("\n").collect().map(fn(v) { v.to_string() })
}

///|
/// Replaces the `"golemcloud/golem_sdk": ...` entry in a JSON deps block.
/// Handles both single-line (`"key": "version"`) and multi-line (`"key": { ... }`) entries.
/// Preserves trailing commas from the original entry.
fn replace_sdk_dep(s : String, replacement : String) -> String {
  let marker = "\"golemcloud/golem_sdk\":"
  let lines = split_lines(s)
  let result : Array[String] = []
  let mut i = 0
  while i < lines.length() {
    let line = lines[i]
    if line.contains(marker) {
      let trimmed = line.trim_start().to_string()
      let indent_len = line.length() - trimmed.length()
      let indent = String::make(indent_len, ' ')
      // Determine if the original entry had a trailing comma
      let mut has_comma = false
      if line.contains("{") && not(line.contains("}")) {
        // Multi-line object: skip to closing brace, check for comma there
        i += 1
        while i < lines.length() {
          let l = lines[i]
          let lt = l.trim_start().to_string()
          if lt.has_prefix("}") {
            has_comma = lt.has_prefix("},")
            break
          }
          i += 1
        }
      } else {
        // Single-line entry: check if line ends with comma
        has_comma = line.trim_end().to_string().has_suffix(",")
      }
      let comma = if has_comma { "," } else { "" }
      result.push("\{indent}\{replacement}\{comma}")
    } else {
      result.push(line)
    }
    i += 1
  }
  result.join("\n")
}

///|
/// Removes the `"bin-deps": { ... }` block from JSON.
/// Handles comma cleanup: removes a trailing comma from the previous line
/// if the removed block was the last property.
fn remove_bin_deps(s : String) -> String {
  let marker = "\"bin-deps\""
  if not(s.contains(marker)) {
    return s
  }
  let lines = split_lines(s)
  let result : Array[String] = []
  let mut i = 0
  while i < lines.length() {
    let line = lines[i]
    if line.contains(marker) {
      // Skip the entire bin-deps block (including closing brace)
      if line.contains("{") && not(line.contains("}")) {
        i += 1
        while i < lines.length() {
          if lines[i].trim_start().to_string().has_prefix("}") {
            break
          }
          i += 1
        }
        // i now points at the closing brace line; skip it too
      }
      // Fix trailing comma on the previous kept line
      if result.length() > 0 {
        let prev = result[result.length() - 1]
        let trimmed = prev.trim_end().to_string()
        if trimmed.has_suffix(",") {
          let indent_part = try! prev[:prev.length() - trimmed.length()]
          let without_comma = try! trimmed[:trimmed.length() - 1]
          result[result.length() - 1] = indent_part.to_string() +
            without_comma.to_string()
        }
      }
    } else {
      result.push(line)
    }
    i += 1
  }
  result.join("\n")
}

///|
/// Inserts a `"bin-deps": { ... }` block before the final closing brace in JSON.
/// Idempotent: returns unchanged if `"bin-deps"` already exists.
fn insert_bin_deps(s : String, entry : String) -> String {
  if s.contains("\"bin-deps\"") {
    return s
  }
  let lines = split_lines(s)
  let result : Array[String] = []
  // Find the last non-empty line that is the top-level closing brace
  let mut closing_idx = -1
  for i = lines.length() - 1; i >= 0; i = i - 1 {
    if lines[i].trim().to_string() == "}" {
      closing_idx = i
      break
    }
  }
  if closing_idx < 0 {
    return s
  }
  for i, line in lines {
    if i == closing_idx {
      // Add comma to previous line if needed
      if result.length() > 0 {
        let prev = result[result.length() - 1]
        let trimmed = prev.trim_end().to_string()
        if not(trimmed.has_suffix(",")) && not(trimmed.has_suffix("{")) {
          let indent_part = try! prev[:prev.length() - trimmed.length()]
          result[result.length() - 1] = indent_part.to_string() + trimmed + ","
        }
      }
      result.push("  \"bin-deps\": {")
      result.push("    \{entry}")
      result.push("  }")
    }
    result.push(line)
  }
  result.join("\n")
}
