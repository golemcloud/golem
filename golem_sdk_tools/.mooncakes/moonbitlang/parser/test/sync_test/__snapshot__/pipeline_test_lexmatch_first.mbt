///|
fn test_firstmatch() -> Unit raise {
  assert_eq(
    lexmatch "ab" {
      ("a|ab" as t, _) => Some(t)
      _ => None
    },
    Some("a"),
  )
}

///|
fn test_search() -> Unit raise {
  assert_eq(
    lexmatch "xxxxxyxxxx" {
      (before, "y", _) => Some(before.length())
      _ => None
    },
    Some(5),
  )
}

fn test_multicases_firstmatch() -> Unit raise {
  fn f(x : String) -> Int {
    lexmatch x {
      (_, "a", _) => 1
      (_, "b", _) => 2
      (_, "c", _) => 3
      _ => -1
    }
  }
  assert_eq(f("xxa"), 1)
  assert_eq(f("xxb"), 2)
  assert_eq(f("xxc"), 3)
  assert_eq(f("xxd"), -1)
}

fn test_nested_captures() -> Unit raise {
  assert_eq(
    lexmatch "abcd" {
      (before, (("b" as b) ("c" as c)) as bc, after) => Some((before, b, c, bc, after))
      _ => None
    },
    Some(("a", 'b', 'c', "bc", "d")),
  )
}

fn test_quantifier() -> Unit raise {
  fn f(x : String) -> Int? {
    lexmatch x {
      (("a{2,4}" as a) "b", _) => Some(a.length())
      _ => None
    }
  }
  assert_eq(
    f("aaab"),
    Some(3),
  )
  assert_eq(f("ab"), None)
}

fn test_non_greedy_quantifier() -> Unit raise {
  fn f(x : String) -> StringView? {
    lexmatch x {
      (("(a|b)*?" as x) "b", _) => Some(x)
      _ => None
    }
  }
  assert_eq(
    f("aaabbb"),
    Some("aaa"),
  )
}

fn test_simple_assertions() -> Unit raise {
  fn f(x : String) -> Bool {
    lexmatch x {
      (_, "a|^b", _) => true
      _ => false
    }
  }
  assert_eq(f("xxa"), true)
  assert_eq(f("xxb"), false)
  assert_eq(f("bxx"), true)
}

///|
fn init {
  println("test_firstmatch: \{try? test_firstmatch()}")
  println("test_search: \{try? test_search()}")
  println("test_multicases_firstmatch: \{try? test_multicases_firstmatch()}")
  println("test_nested_captures: \{try? test_nested_captures()}")
  println("test_quantifier: \{try? test_quantifier()}")
  println("test_non_greedy_quantifier: \{try? test_non_greedy_quantifier()}")
  println("test_simple_assertions: \{try? test_simple_assertions()}")
}
