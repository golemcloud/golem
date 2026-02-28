///|
let fixtures_path = "test/mbti_parser_test/fixtures"

///|
fn @test.Test::run(t : Self, filename : String) -> Unit raise {
  let content = @fs.read_file_to_string(
    fixtures_path + "/" + filename + ".mbti",
  )
  let lex_result = @lexer.tokens_from_string(
    name=filename,
    content,
    comment=false,
  )
  let mbti = @mbti_parser.mbti(
    lex_result.tokens.filter(fn(triple) {
      not(triple.0 is (NEWLINE | COMMENT(_)))
    }),
    initial_pos=@basic.Position::{ fname: filename, lnum: 1, bol: 0, cnum: 0 },
  )
  ignore(mbti)
  // t.write(mbti.to_json().stringify(indent=2))
  t.snapshot(filename=filename + ".json")
}
