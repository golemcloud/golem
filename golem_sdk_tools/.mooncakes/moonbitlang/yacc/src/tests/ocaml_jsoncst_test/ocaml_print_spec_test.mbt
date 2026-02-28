///|
test "parser" (it : @test.Test) {
  let lexer = @parser.new_lexer(ocaml_parser_spec_src)
  fn token() {
    try! lexer.next_token()
  }

  let spec = @parser.spec(token, 0)
  let buf = StringBuilder::new()
  @ast.print_spec(spec, buf)
  it.write(buf.to_string())
  it.snapshot(filename="ocaml.mly")
}
