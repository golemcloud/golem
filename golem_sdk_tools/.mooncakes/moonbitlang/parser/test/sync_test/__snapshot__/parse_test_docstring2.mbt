// Note:
//
// Doc comments in local or before test block, fn init, and fn main are useless.
// The doc_search should discard them correctly; otherwise, it will cause the 
// formatter to print them in the wrong place.
//

/// Useless doc comment, shouldn't appear in AST
test {
  /// Useless doc comment, shouldn't appear in AST
  fn f() {

  }

  /// Useless doc comment, shouldn't appear in AST
  fn g() {

  }
}

/// comment for h1
fn h1() -> Unit {

}

/// Useless doc comment, shouldn't appear in AST
fn init {
  /// Useless doc comment, shouldn't appear in AST
  fn f() {

  }

  /// Useless doc comment, shouldn't appear in AST
  fn g() {

  }
}

/// comment for h2
fn h2() -> Unit {

}

/// Useless doc comment, shouldn't appear in AST
fn main {
  /// Useless doc comment, shouldn't appear in AST
  fn f() {

  }

  /// Useless doc comment, shouldn't appear in AST
  fn g() {

  }
}

/// comment for h3
fn h3() -> Unit {

}



