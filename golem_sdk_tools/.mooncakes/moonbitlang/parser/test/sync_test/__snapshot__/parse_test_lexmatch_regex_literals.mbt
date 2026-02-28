fn main {
  let x = 1
  lexmatch s with longest {
    (re"ab$", _) => ()
    (re"a\\{x}", _) => ()
    (re"a\{x}", _) => ()
    ("a\{x}", _) => ()
    (("a"), _) => ()
    (("a" "b"), _) => ()
    ((1), _) => ()
    1 => ()
    _ => ()
  }
}
