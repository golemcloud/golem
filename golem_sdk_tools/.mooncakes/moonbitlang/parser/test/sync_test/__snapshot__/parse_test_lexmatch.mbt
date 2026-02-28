fn init {
  // well semantic
  lexmatch s with longest {
    "a" => ()
    ("a", _) => ()
    _ => ()
  }
  lexmatch s {
    "a" => ()
    (_, "a") => ()
    (_, "a", _) => ()
    ...
  }
  lexmatch s {
    ("a" as a) => ()
    ("a" "b") => ()
    ("a" ("b" as b)) => ()
    ("a" "b", c) => ()
    (("a" as a) "b", c) => ()
    s2 => ()
  }
  // ill semantic should be accepted by parser as well
  lexmatch s with xxx {
    ("", "", "") => ()
    (a, b, c) => ()
  }
}
