///|
fn test_unicode_support() -> Unit raise {
  // Test Unicode character matching
  fn f(str : String) -> Int {
    lexmatch str with longest {
      "(我❤️中国|𠮷)" => 1
      ("[\u3041-\u3096\u3099-\u309f]{3}" as s, _) => {
        println(s)
        2
      }
      _ => 3
    }
  }

  assert_eq(f("我❤️中国"), 1)
  assert_eq(f("𠮷"), 1)
  assert_eq(f("あいうえお"), 2)
  assert_eq(f("abc"), 3)
}

///|
fn test_unicode_support2() -> Unit raise {
  // Test Unicode character matching
  fn f(str : String) -> Char raise {
    lexmatch str with longest {
      ("." as c, _) => c
      _ => raise Failure::Failure("No match")
    }
  }

  assert_eq(f("我❤️中国"), '我')
  assert_eq(f("❤️"), '❤')
  assert_eq(f("𠮷"), '𠮷')
  assert_eq(f("あいうえお"), 'あ')
  assert_eq(f("abc"), 'a')
}

///|
fn main {
  println("test_unicode_support")
  println(try? test_unicode_support())
  println("test_unicode_support2")
  println(try? test_unicode_support2())
}
