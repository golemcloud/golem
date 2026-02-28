fn main {
  lexmatch s {
    (re"ab$"; re"cd$", _) => ()
    (re"a\{x}" re"b", _) => ()
    _ => ()
  }
}
