

fn lex_suffix(str : StringView) -> StringView raise {
  lexmatch str with longest {
    (".*" "\." (".*$" as ext), _) => ext
    _ => fail("Invalid suffix")
  }  
}

fn main {
  println(try? lex_suffix("test.txt"))
  println(try? lex_suffix("test.txt.tar.gz"))
  println(try? lex_suffix("test.txt.tar.gz.tar.gz"))
  println(try? lex_suffix("test.txt.tar.gz.tar.gz.tar.gz"))
  println(try? lex_suffix("test.txt.tar.gz.tar.gz.tar.gz.tar.gz"))
  println(try? lex_suffix("test.txt.tar.gz.tar.gz.tar.gz.tar.gz.tar.gz"))
  println(try? lex_suffix("test.txt.tar.gz.tar.gz.tar.gz.tar.gz.tar.gz"))
}
