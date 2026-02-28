fn init {
  let s = "abc"
  lexmatch (try! s[0:1]) with longest {
    ("a", rest) => println(rest)
    _ => ()
  }
}
