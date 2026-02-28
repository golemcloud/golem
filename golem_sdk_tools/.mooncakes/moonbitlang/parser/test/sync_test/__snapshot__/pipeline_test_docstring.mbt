/// Enum representing h with parameters M and N.
enum H[M, N] {
  A
  D
  E (M)
  F (N)
}

/// `foo(x: Int)`
///
/// A simple function that takes an `Int` and returns a `String` representing
/// the answer to life, the universe, and everything.
///
/// # Examples
/// ```moonbit
/// let answer = foo(42)
/// println(answer) // prints "42"
/// ```
fn foo(x : Int) -> String {
  "42"
}

/// `inc(x: Int)`
///
/// A simple function that takes an `Int` and return its successor.
let inc : (Int) -> Int = fn(x) { x + 1 }

// Initializes and demonstrates the functions.
fn init {
  // The inner function f.
  // @param {h} x - The input parameter of type h.
  let f = fn(x) {
    match x {
      // Case A in enum h.
      H::A => println('a')
      // Case E in enum h.
      // @param {Int} x - The input parameter of type Int.
      E((x:Int)) => println(x)
      // Case F in enum h.
      // @param {N} c - The input parameter of type N.
      F(c) => println('f')
    }
  }
  f(E(1)) // Example 1: Call f with E(1).
  f(A)    // Example 2: Call f with A.
  println(foo(1))

  println(inc(42)) // Example 3: Call inc with 42.
  println(dec(42))
}


/// `dec(x: Int)`
///
/// A simple function that takes an `Int` and return its predecessor.
let dec : (Int) -> Int = fn(x) { x - 1 }
