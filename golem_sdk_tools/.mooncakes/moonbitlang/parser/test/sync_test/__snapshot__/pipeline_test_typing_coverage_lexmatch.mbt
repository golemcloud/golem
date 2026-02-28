fn tokenize(input : BytesView) -> Array[BytesView] {
  lexmatch input with longest {
    ("[a-z]+" as word, rest) => [word, ..tokenize(rest)]
    ("[0-9]+" as digits, rest) => [digits, ..tokenize(rest)]
    ("[ \t\n]+", rest) => tokenize(rest)
    "" => []
    _ => []
  }
}

fn init {
  let tokens = tokenize(b"ab12 \ncd3")
  let _ = tokens.length()
  ()
}
