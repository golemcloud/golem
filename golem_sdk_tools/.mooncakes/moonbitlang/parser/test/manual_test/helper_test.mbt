///|
/// Verifies that both the hand-rolled parser and the moonyacc parser produce identical ASTs,
/// and that no diagnostics are generated for all test cases in `__snapshot__`.
fn @test.Test::run(t : Self) -> Unit raise Error {
  let name = t.name()
  let source_file = "test/manual_test/__snapshot__/\{name}.mbt"
  let source = @fs.read_file_to_string(source_file) catch {
    err => fail("failed to read file \{name}.mbt: \{err}")
  }
  let hand_result = @parser.parse_string(source, name~, parser=Handrolled)
  let yacc_result = @parser.parse_string(source, name~, parser=MoonYacc)
  if !(hand_result.1.is_empty() && yacc_result.1.is_empty()) {
    let buf = StringBuilder::new()
    let _ : Unit = @basic.show_loc.protect(String, fn() {
      buf
      ..write_string("parsing \{name}.mbt failed due to diagnostics.\n")
      ..write_string("Handrolled parser diagnostics: ")
      ..write_string(hand_result.1.to_json().stringify(indent=2))
      ..write_string("\nYacc parser diagnostics: ")
      ..write_string(yacc_result.1.to_json().stringify(indent=2))
      .write_string("\n")
    })
    fail(buf.to_string())
  }
  let hand_json = hand_result.0.map(impl_ => impl_.json_repr()).to_json()
  let yacc_json = yacc_result.0.map(impl_ => impl_.json_repr()).to_json()
  if hand_json != yacc_json {
    @fs.create_dir(debug_dir)
    @fs.write_string_to_file("\{debug_dir}/\{name}.mbt", source)
    @fs.write_string_to_file(
      "\{debug_dir}/\{name}_hand.json",
      hand_json.stringify(indent=2),
    )
    @fs.write_string_to_file(
      "\{debug_dir}/\{name}_yacc.json",
      yacc_json.stringify(indent=2),
    )
    fail(
      "parsing \{name}.mbt produced different ASTs between moonyacc and handrolled parsers.",
    )
  }
  let json : Json = hand_json
  t.write(json.stringify(indent=2))
  t.snapshot(filename="\{name}.json")
}

///|
let debug_dir = "test/manual_test/debug"

///|
fn init {
  try {
    if @fs.path_exists(debug_dir) {
      @fs.remove_dir(debug_dir)
    }
  } catch {
    e => println(e)
  }
}
