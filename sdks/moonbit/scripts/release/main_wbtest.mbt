///|
fn transform_error(f : () -> Unit raise ReleaseTransformError) -> String {
  try f() catch {
    ReleaseTransformError(message) => message
  } noraise {
    _ => "NO ERROR"
  }
}

///|
test "SDK dependency replacement rejects another import on the managed line" {
  let module_text =
    #|name = "example"
    #|
    #|import {
    #|  "golemcloud/golem_sdk@0.5.1", "example/other@1.0.0",
    #|}
    #|
  inspect(
    transform_error(() => ignore(replace_sdk_dep(module_text, "0.6.0"))),
    content="Invalid existing SDK version: 0.5.1\", \"example/other@1.0.0",
  )
}

///|
test "binary dependency removal rejects another entry in the managed object" {
  let module_text =
    #|name = "example"
    #|
    #|options(
    #|  "bin-deps": { "golemcloud/golem_sdk_tools": "0.5.1", "example/other": "1.0.0" },
    #|)
    #|
  inspect(
    transform_error(() => ignore(remove_bin_deps(module_text))),
    content="Invalid existing tools version: 0.5.1\", \"example/other\": \"1.0.0",
  )
}

///|
/// The transformations count how often each switched path occurs, so any edit to the
/// example manifest that adds or drops one of them silently breaks the release script.
/// Round-tripping the real manifest keeps those counts honest.
test "path transformation round-trips the example manifest" {
  let dev = @fs.read_file_to_string(golem_yaml_path)
  let released = transform_yaml_paths(dev, true)
  assert_not_eq(released, dev)
  inspect(transform_yaml_paths(released, false) == dev, content="true")
}
