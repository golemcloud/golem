///|
let example_dir = "../golem_sdk_example1"

///|
let mod_json_path = "\{example_dir}/moon.mod.json"

///|
let golem_yaml_path = "\{example_dir}/golem.yaml"

///|
let build_sh_path = "\{example_dir}/build.sh"

///|
let sdk_mooncakes_path = ".mooncakes/golemcloud/golem_sdk"

///|
let tools_mooncakes_run = "moon run -C .mooncakes/golemcloud/golem_sdk_tools cmd"

///|
let dev_reexports_prefix = "command: moon run cmd -- reexports ../golem_sdk ../golem_sdk_example1/"

///|
let dev_agents_prefix = "command: moon run cmd -- agents ../golem_sdk_example1/"

///|
let dev_dir_line = "            dir: ../golem_sdk_tools"

///|
let dev_build_sh_reexports = "(cd ../golem_sdk_tools && moon run cmd -- reexports ../golem_sdk ../golem_sdk_example1/golem_moonbit_examples)"

///|
let dev_build_sh_agents = "(cd ../golem_sdk_tools && moon run cmd -- agents ../golem_sdk_example1/golem_moonbit_examples)"

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
  let rel_reexports = "command: \{tools_mooncakes_run} -- reexports \{sdk_mooncakes_path} "
  let rel_agents = "command: \{tools_mooncakes_run} -- agents "
  let rel_build_reexports = "\{tools_mooncakes_run} -- reexports \{sdk_mooncakes_path} golem_moonbit_examples"
  let rel_build_agents = "\{tools_mooncakes_run} -- agents golem_moonbit_examples"
  // 1. moon.mod.json — compute new content
  let mod_json = @fs.read_file_to_string(mod_json_path)
  let mut m = replace_sdk_dep(
    mod_json,
    "\"golemcloud/golem_sdk\": \"\{sdk_version}\"",
  )
  m = remove_bin_deps(m)
  m = insert_bin_deps(m, "\"golemcloud/golem_sdk_tools\": \"\{tools_version}\"")
  // 2. golem.yaml — compute new content
  let yaml = @fs.read_file_to_string(golem_yaml_path)
  let mut y = remove_lines_containing(yaml, "dir: ../golem_sdk_tools")
  y = y.replace_all(old=dev_reexports_prefix, new=rel_reexports)
  y = y.replace_all(old=dev_agents_prefix, new=rel_agents)
  // 3. build.sh — compute new content
  let build = @fs.read_file_to_string(build_sh_path)
  let mut b = build.replace_all(
    old=dev_build_sh_reexports,
    new=rel_build_reexports,
  )
  b = b.replace_all(old=dev_build_sh_agents, new=rel_build_agents)
  // Write all files only after all transforms succeed
  @fs.write_string_to_file(mod_json_path, m)
  @fs.write_string_to_file(golem_yaml_path, y)
  @fs.write_string_to_file(build_sh_path, b)
  println("")
  println("Done. Files modified:")
  println(
    "  \{mod_json_path}  — golem_sdk dep: \{sdk_version}, golem_sdk_tools bin-dep: \{tools_version}",
  )
  println(
    "  \{golem_yaml_path} — build commands use mooncakes-installed tools",
  )
  println(
    "  \{build_sh_path}   — build commands use mooncakes-installed tools",
  )
  println("")
  println("Review the changes, then copy \{example_dir}/ as your template.")
  println("To revert: moon run -C scripts release -- --dev")
}

///|
fn revert_to_dev() -> Unit raise {
  println("==> Reverting to development mode...")
  let rel_reexports = "command: \{tools_mooncakes_run} -- reexports \{sdk_mooncakes_path} "
  let rel_agents = "command: \{tools_mooncakes_run} -- agents "
  let rel_build_reexports = "\{tools_mooncakes_run} -- reexports \{sdk_mooncakes_path} golem_moonbit_examples"
  let rel_build_agents = "\{tools_mooncakes_run} -- agents golem_moonbit_examples"
  let path_dep = "\"golemcloud/golem_sdk\": {\n      \"path\": \"../golem_sdk\"\n    }"
  // 1. moon.mod.json — compute new content
  let mod_json = @fs.read_file_to_string(mod_json_path)
  let mut m = replace_sdk_dep(mod_json, path_dep)
  m = remove_bin_deps(m)
  // 2. golem.yaml — compute new content
  let yaml = @fs.read_file_to_string(golem_yaml_path)
  let mut y = yaml.replace_all(old=rel_reexports, new=dev_reexports_prefix)
  y = y.replace_all(old=rel_agents, new=dev_agents_prefix)
  y = insert_dir_after_command(y, "moon run cmd -- reexports")
  y = insert_dir_after_command(y, "moon run cmd -- agents")
  // 3. build.sh — compute new content
  let build = @fs.read_file_to_string(build_sh_path)
  let mut b = build.replace_all(
    old=rel_build_reexports,
    new=dev_build_sh_reexports,
  )
  b = b.replace_all(old=rel_build_agents, new=dev_build_sh_agents)
  // Write all files only after all transforms succeed
  @fs.write_string_to_file(mod_json_path, m)
  @fs.write_string_to_file(golem_yaml_path, y)
  @fs.write_string_to_file(build_sh_path, b)
  println("Done. Now in development mode (local path deps).")
}

// --- String helpers ---

///|
fn split_lines(s : String) -> Array[String] {
  s.split("\n").collect().map(fn(v) { v.to_string() })
}

///|
fn remove_lines_containing(s : String, needle : String) -> String {
  let lines = split_lines(s)
  let kept : Array[String] = []
  for line in lines {
    if not(line.contains(needle)) {
      kept.push(line)
    }
  }
  kept.join("\n")
}

///|
/// Inserts a `dir:` line after YAML command lines containing `needle`.
/// Idempotent: skips insertion if the next line is already the dir line.
fn insert_dir_after_command(s : String, needle : String) -> String {
  let lines = split_lines(s)
  let result : Array[String] = []
  for i, line in lines {
    result.push(line)
    if line.trim_start().to_string().has_prefix("- command:") &&
      line.contains(needle) {
      // Check if next line is already the dir line
      let next_is_dir = i + 1 < lines.length() &&
        lines[i + 1].trim_start().to_string() == "dir: ../golem_sdk_tools"
      if not(next_is_dir) {
        result.push(dev_dir_line)
      }
    }
  }
  result.join("\n")
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
