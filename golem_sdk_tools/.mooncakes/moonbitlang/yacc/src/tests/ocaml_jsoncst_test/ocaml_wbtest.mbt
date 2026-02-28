///|
test (it : @test.Test) {
  let lexbuf = @lexbuf.StringLexbuf::from_string(binary_search_ml_str)
  let json = implementation(fn() { token(lexbuf) }, 0)
  it.writeln(json.stringify(indent=2))
  it.snapshot(filename="binary_search_ml.json")
}
