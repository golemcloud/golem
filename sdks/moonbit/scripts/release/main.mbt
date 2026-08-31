///|
let example_dir = "../golem_sdk_example1"

///|
let mod_path = "\{example_dir}/moon.mod"

///|
let golem_yaml_path = "\{example_dir}/golem.yaml"

///|
let sdk_mooncakes_path = "{{ appRootDir }}/.mooncakes/golemcloud/golem_sdk"

///|
let tools_mooncakes_path = "{{ appRootDir }}/.mooncakes/golemcloud/golem_sdk_tools"

///|
let dev_tools_path = "{{ appRootDir }}/../golem_sdk_tools"

///|
let dev_sdk_path = "{{ appRootDir }}/../golem_sdk"

///|
let dev_debug_build_path = "{{ appRootDir }}/../_build/wasm/debug/build/golemcloud/golem_sdk_example1/"

///|
let release_debug_build_path = "{{ appRootDir }}/_build/wasm/debug/build/"

///|
let dev_release_build_path = "{{ appRootDir }}/../_build/wasm/release/build/golemcloud/golem_sdk_example1/"

///|
let release_release_build_path = "{{ appRootDir }}/_build/wasm/release/build/"

///|
let dev_sdk_version = "0.5.1"

///|
priv suberror ReleaseTransformError {
  ReleaseTransformError(String)
} derive(Debug)

///|
fn release_fail(message : String) -> Unit raise ReleaseTransformError {
  raise ReleaseTransformError(message)
}

///|
fn main {
  let args = @env.args()
  if args.length() < 2 {
    print_usage()
    @sys.exit(1)
    return
  }
  if args[1] == "--dev" {
    revert_to_dev() catch {
      e => {
        println("Error: \{e}")
        @sys.exit(1)
      }
    }
  } else {
    let sdk_version = args[1]
    let tools_version = if args.length() >= 3 { args[2] } else { sdk_version }
    prepare_release(sdk_version, tools_version) catch {
      e => {
        println("Error: \{e}")
        @sys.exit(1)
      }
    }
  }
}

///|
fn print_usage() -> Unit {
  let usage =
    #|Usage: moon -C scripts run release -- <sdk-version> [tools-version]
    #|       moon -C scripts run release -- --dev
    #|
    #|Examples:
    #|  moon -C scripts run release -- 0.1.0            (same version for SDK and tools)
    #|  moon -C scripts run release -- 0.1.0 0.2.0      (different versions)
    #|  moon -C scripts run release -- --dev             (revert to development mode)
  println(usage)
}

///|
fn prepare_release(sdk_version : String, tools_version : String) -> Unit raise {
  validate_version("SDK", sdk_version)
  validate_version("tools", tools_version)
  println(
    "==> Preparing template release with golem_sdk \{sdk_version}, golem_sdk_tools \{tools_version}...",
  )
  // Compute every transformation before writing either file.
  let mod_ = @fs.read_file_to_string(mod_path)
  let mut m = replace_sdk_dep(mod_, sdk_version)
  m = remove_bin_deps(m)
  m = insert_bin_deps(m, tools_version)
  let yaml = @fs.read_file_to_string(golem_yaml_path)
  let y = transform_yaml_paths(yaml, true)
  @fs.write_string_to_file(mod_path, m)
  @fs.write_string_to_file(golem_yaml_path, y)
  println("")
  println("Done. Files modified:")
  println(
    "  \{mod_path}  — golem_sdk dep: \{sdk_version}, golem_sdk_tools bin-dep: \{tools_version}",
  )
  println(
    "  \{golem_yaml_path} — build commands use mooncakes-installed dependencies and standalone build paths",
  )
  println("")
  println("Review the changes, then copy \{example_dir}/ as your template.")
  println("To revert: moon -C scripts run release -- --dev")
}

///|
fn revert_to_dev() -> Unit raise {
  println("==> Reverting to development mode...")
  // Compute every transformation before writing either file.
  let mod_ = @fs.read_file_to_string(mod_path)
  let mut m = replace_sdk_dep(mod_, dev_sdk_version)
  m = remove_bin_deps(m)
  let yaml = @fs.read_file_to_string(golem_yaml_path)
  let y = transform_yaml_paths(yaml, false)
  @fs.write_string_to_file(mod_path, m)
  @fs.write_string_to_file(golem_yaml_path, y)
  println("Done. Now in development mode (local workspace dependency).")
}

// --- String helpers ---

///|
fn split_lines(s : String) -> Array[String] {
  s.split("\n").collect().map(fn(v) { v.to_owned() })
}

///|
fn validate_version(
  label : String,
  version : String,
) -> Unit raise ReleaseTransformError {
  if version == "" || version.contains("\"") || version.contains("\n") {
    release_fail("Invalid \{label} version: \{version}")
  }
}

///|
fn count_occurrences(s : String, marker : String) -> Int {
  s.split(marker).collect().length() - 1
}

///|
/// Replaces the active MoonBit module's single SDK import while preserving its indentation.
fn replace_sdk_dep(
  s : String,
  version : String,
) -> String raise ReleaseTransformError {
  let marker = "\"golemcloud/golem_sdk@"
  let lines = split_lines(s)
  let result : Array[String] = []
  let mut replaced = 0
  for line in lines {
    if line.contains(marker) {
      replaced += 1
      let trimmed = line.trim_start().to_owned()
      if !trimmed.has_prefix(marker) ||
        !(trimmed.has_suffix("\"") || trimmed.has_suffix("\",")) {
        release_fail("Unsupported golem_sdk import syntax in \{mod_path}")
      }
      let indent_len = line.length() - trimmed.length()
      let indent = String::make(indent_len, ' ')
      let comma = if trimmed.has_suffix(",") { "," } else { "" }
      let suffix = "\"" + comma
      let existing_version = trimmed[marker.length():trimmed.length() -
      suffix.length()].to_owned()
      validate_version("existing SDK", existing_version)
      if trimmed != marker + existing_version + suffix {
        release_fail("Unsupported golem_sdk import syntax in \{mod_path}")
      }
      result.push("\{indent}\"golemcloud/golem_sdk@\{version}\"\{comma}")
    } else {
      result.push(line)
    }
  }
  if replaced != 1 {
    release_fail(
      "Expected exactly one golem_sdk import in \{mod_path}, found \{replaced}",
    )
  }
  result.join("\n")
}

///|
/// Removes the standalone template's generated tool binary dependency block.
fn remove_bin_deps(s : String) -> String raise ReleaseTransformError {
  let marker = "\"bin-deps\""
  if !s.contains(marker) {
    return s
  }
  let lines = split_lines(s)
  let mut marker_idx = -1
  for i, line in lines {
    if line.contains(marker) {
      if marker_idx >= 0 {
        release_fail("Found multiple bin-deps entries in \{mod_path}")
      }
      marker_idx = i
    }
  }
  if marker_idx <= 0 || marker_idx + 1 >= lines.length() {
    release_fail("Unsupported bin-deps block in \{mod_path}")
  }
  let options_idx = marker_idx - 1
  let closing_idx = marker_idx + 1
  let entry = lines[marker_idx].trim().to_owned()
  let entry_prefix = "\"bin-deps\": { \"golemcloud/golem_sdk_tools\": \""
  let entry_suffix = "\" },"
  if lines[options_idx].trim().to_owned() != "options(" ||
    lines[closing_idx].trim().to_owned() != ")" ||
    !entry.has_prefix(entry_prefix) ||
    !entry.has_suffix(entry_suffix) {
    release_fail("Unsupported bin-deps block in \{mod_path}")
  }
  let existing_version = entry[entry_prefix.length():entry.length() -
  entry_suffix.length()].to_owned()
  validate_version("existing tools", existing_version)
  if entry != entry_prefix + existing_version + entry_suffix {
    release_fail("Unsupported bin-deps block in \{mod_path}")
  }
  let start_idx = if options_idx > 0 && lines[options_idx - 1] == "" {
    options_idx - 1
  } else {
    options_idx
  }
  let result : Array[String] = []
  for i, line in lines {
    if i < start_idx || i > closing_idx {
      result.push(line)
    }
  }
  result.join("\n")
}

///|
/// Adds the binary dependency consumed by `moon run cmd` in a standalone template.
fn insert_bin_deps(
  s : String,
  tools_version : String,
) -> String raise ReleaseTransformError {
  if s.contains("\"bin-deps\"") {
    release_fail(
      "Refusing to overwrite an unmanaged bin-deps block in \{mod_path}",
    )
  }
  if s.contains("options(") {
    release_fail(
      "Refusing to merge bin-deps into an unmanaged options block in \{mod_path}",
    )
  }
  if !s.has_suffix("\n") {
    release_fail("Expected \{mod_path} to end with a newline")
  }
  s +
  "\noptions(\n  \"bin-deps\": { \"golemcloud/golem_sdk_tools\": \"\{tools_version}\" },\n)\n"
}

///|
fn switch_path(
  s : String,
  from : String,
  to : String,
  expected : Int,
  label : String,
) -> String raise ReleaseTransformError {
  let from_count = count_occurrences(s, from)
  let to_count = count_occurrences(s, to)
  if from_count == expected && to_count == 0 {
    s.replace_all(old=from, new=to)
  } else if from_count == 0 && to_count == expected {
    s
  } else {
    release_fail(
      "Unexpected \{label} paths in \{golem_yaml_path}: found \{from_count} source and \{to_count} target occurrences; expected \{expected} of exactly one form",
    )
    abort("")
  }
}

///|
fn transform_yaml_paths(
  yaml : String,
  release : Bool,
) -> String raise ReleaseTransformError {
  let from_tools = if release { dev_tools_path } else { tools_mooncakes_path }
  let to_tools = if release { tools_mooncakes_path } else { dev_tools_path }
  let from_sdk_arg = if release {
    dev_sdk_path + "\""
  } else {
    sdk_mooncakes_path + "\""
  }
  let to_sdk_arg = if release {
    sdk_mooncakes_path + "\""
  } else {
    dev_sdk_path + "\""
  }
  let from_sdk_wit = if release {
    dev_sdk_path + "/wit"
  } else {
    sdk_mooncakes_path + "/wit"
  }
  let to_sdk_wit = if release {
    sdk_mooncakes_path + "/wit"
  } else {
    dev_sdk_path + "/wit"
  }
  let from_debug_build = if release {
    dev_debug_build_path
  } else {
    release_debug_build_path
  }
  let to_debug_build = if release {
    release_debug_build_path
  } else {
    dev_debug_build_path
  }
  let from_release_build = if release {
    dev_release_build_path
  } else {
    release_release_build_path
  }
  let to_release_build = if release {
    release_release_build_path
  } else {
    dev_release_build_path
  }
  let mut result = switch_path(
    yaml, from_tools, to_tools, 12, "golem_sdk_tools",
  )
  result = switch_path(
    result, from_sdk_arg, to_sdk_arg, 4, "golem_sdk argument",
  )
  result = switch_path(result, from_sdk_wit, to_sdk_wit, 4, "golem_sdk WIT")
  result = switch_path(
    result, from_debug_build, to_debug_build, 2, "debug MoonBit build",
  )
  result = switch_path(
    result, from_release_build, to_release_build, 2, "release MoonBit build",
  )
  result
}
