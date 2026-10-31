///|
let example_dir = "../golem_sdk_example1"

///|
let mod_path = "\{example_dir}/moon.mod"

///|
let golem_yaml_path = "\{example_dir}/golem.yaml"

///|
let sdk_mooncakes_path = "{{ appRootDir }}/.mooncakes/golemcloud/golem_sdk"

///|
let release_tools_path = "{{ appRootDir }}"

///|
let dev_tools_path = "{{ appRootDir }}/../golem_sdk_tools"

///|
let dev_sdk_path = "{{ appRootDir }}/../golem_sdk"

///|
/// In the workspace the example is a `moon.work` member, so `moon` nests its artifacts
/// under the module name; a standalone template builds them directly under `build/`.
/// Both forms keep the `{{ moonbitBuildPackagePath }}` suffix so that neither is a
/// prefix of the other and `switch_path` can tell them apart.
let dev_debug_build_path = "{{ appRootDir }}/_build/wasm/debug/build/golemcloud/golem_sdk_example1/{{ moonbitBuildPackagePath }}"

///|
let release_debug_build_path = "{{ appRootDir }}/_build/wasm/debug/build/{{ moonbitBuildPackagePath }}"

///|
let dev_release_build_path = "{{ appRootDir }}/_build/wasm/release/build/golemcloud/golem_sdk_example1/{{ moonbitBuildPackagePath }}"

///|
let release_release_build_path = "{{ appRootDir }}/_build/wasm/release/build/{{ moonbitBuildPackagePath }}"

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
/// The default rendering of an error only shows its constructor path, which hides
/// the diagnostics the transformations raise.
fn report_error(e : Error) -> Unit {
  match e {
    ReleaseTransformError(message) => println("Error: \{message}")
    _ => println("Error: \{e}")
  }
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
        report_error(e)
        @sys.exit(1)
      }
    }
  } else {
    let sdk_version = args[1]
    let tools_version = if args.length() >= 3 { args[2] } else { sdk_version }
    prepare_release(sdk_version, tools_version) catch {
      e => {
        report_error(e)
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
  let m = replace_sdk_dep(mod_, sdk_version)
  let yaml = @fs.read_file_to_string(golem_yaml_path)
  let y = transform_yaml_paths(yaml, true, tools_version~)
  @fs.write_string_to_file(mod_path, m)
  @fs.write_string_to_file(golem_yaml_path, y)
  println("")
  println("Done. Files modified:")
  println("  \{mod_path}  — golem_sdk dep: \{sdk_version}")
  println(
    "  \{golem_yaml_path} — build commands use moonx with tools version \{tools_version} and standalone build paths",
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
  let m = replace_sdk_dep(mod_, dev_sdk_version)
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
fn count_occurrences(s : String, marker : String) -> Int32 {
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
fn switch_path(
  s : String,
  from : String,
  to : String,
  expected : Int32,
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
  tools_version~ : String = dev_sdk_version,
) -> String raise ReleaseTransformError {
  let from_tools = "dir: \"" +
    (if release { dev_tools_path } else { release_tools_path }) +
    "\""
  let to_tools = "dir: \"" +
    (if release { release_tools_path } else { dev_tools_path }) +
    "\""
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
  // 2 presets x 3 tool invocations
  let mut result = switch_path(yaml, from_tools, to_tools, 6, "golem_sdk_tools")
  let local_command = "moon run cmd --"
  let registry_prefix = "moonx golemcloud/golem_sdk_tools/cmd@"
  let lines = split_lines(result)
  let mut tool_commands = 0
  for i, line in lines {
    if line.contains(local_command) {
      tool_commands += 1
      if release {
        lines[i] = line.replace_all(
          old=local_command,
          new="\{registry_prefix}\{tools_version} --",
        )
      }
    } else if line.contains(registry_prefix) {
      tool_commands += 1
      let parts = line.split(registry_prefix).collect()
      let rest = parts[1].split(" --").collect()
      if parts.length() != 2 || rest.length() < 2 {
        release_fail("Unsupported moonx command in \{golem_yaml_path}")
      }
      validate_version("existing tools", rest[0].to_owned())
      let command = if release {
        "\{registry_prefix}\{tools_version} --"
      } else {
        local_command
      }
      let arguments = [
        for j in 1..<rest.length() => rest[j].to_owned()
      ].join(" --")
      lines[i] = parts[0].to_owned() + command + arguments
    }
  }
  if tool_commands != 4 {
    release_fail(
      "Expected 4 tool commands in \{golem_yaml_path}, found \{tool_commands}",
    )
  }
  result = lines.join("\n")
  // 2 presets x the `reexports` SDK argument
  result = switch_path(
    result, from_sdk_arg, to_sdk_arg, 2, "golem_sdk argument",
  )
  // Each preset lists the WIT path as an argument and a source.
  result = switch_path(result, from_sdk_wit, to_sdk_wit, 4, "golem_sdk WIT")
  // The embedded core wasm is an argument and a source in each preset.
  result = switch_path(
    result, from_debug_build, to_debug_build, 2, "debug MoonBit build",
  )
  result = switch_path(
    result, from_release_build, to_release_build, 2, "release MoonBit build",
  )
  result
}
