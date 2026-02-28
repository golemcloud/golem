// Test cases for static data lifting pass

// Basic tuple lifting
fn test_basic_tuple() -> Unit {
  let x = (1, 2, 3)
  println(x)
}

// Inline tuple lifting
fn test_inline_tuple() -> Unit {
  println((4, 5, 6))
}

// Nested tuple lifting
fn test_nested_tuple() -> Unit {
  let nested = ((1, 2), (3, 4))
  println(nested)
}

// String constant lifting
fn test_string_constant() -> Unit {
  let msg = "hello world"
  println(msg)
}

// Mixed constant tuple
fn test_mixed_tuple() -> Unit {
  let mixed = (42, "test", true)
  println(mixed)
}

// Record lifting (immutable fields only)
struct Point {
  x : Int
  y : Int
} derive(Show)

fn test_record_lifting() -> Unit {
  let p : Point = { x: 10, y: 20 }
  println("Point: \{p.x}, \{p.y}")
  println(p)
}

// Constructor lifting
enum Color {
  Red
  Green  
  Blue
  RGB(Int, Int, Int)
} derive(Show)

fn test_constructor_lifting() -> Unit {
  let red = Red
  let green = Green
  let blue = Blue
  let custom = RGB(255, 128, 0)
  println(red)
  println(green)
  println(blue)
  println(custom)
}

// Complex nested structure
struct ComplexStruct {
  x : (Int, Int)
  y : Color
} derive(Show)

fn test_complex_structure() -> Unit {
  let complex : ComplexStruct = {
    x: (1, 2),
    y: RGB(100, 200, 50)
  }
  println(complex)
}

// Test that should NOT be lifted - runtime dependency
fn test_no_lift_runtime(n : Int) -> Unit {
  let runtime_tuple = (n, n + 1, n + 2)  // Should not lift
  println(runtime_tuple)
}

// Test that should NOT be lifted - mutable struct
struct MutablePoint {
  mut x : Int
  y : Int
} derive(Show)

fn test_no_lift_mutable() -> Unit {
  let mp : MutablePoint = { x: 1, y: 2 }  // Should not lift due to mutable field
  mp.x = 10  // Use the mutability
  println("Mutable point: \{mp.x}, \{mp.y}")
  println(mp)
}

// Test that should NOT be lifted - empty tuple
fn test_no_lift_empty() -> Unit {
  let empty = ()  // Should not lift (empty)
  println("Empty tuple")
  println(empty)
}

// Test static data referencing other static data
fn test_referencing_static() -> Unit {
  let base = (1, 2)
  let extended = (base, 3)  // Should lift since base is static
  println(extended)
}

// Test multiple references to same static data
fn test_multiple_references() -> Unit {
  let data1 = (7, 8, 9)
  let data2 = (7, 8, 9)  // Same content, should create separate lifted bindings
  println(data1)
  println(data2)
}

// Test in different scopes
fn test_different_scopes() -> Unit {
  {
    let local1 = (10, 11)
    println(local1)
  }
  {
    let local2 = (12, 13)  
    println(local2)
  }
}

// Test with constants
fn test_with_constants() -> Unit {
  let pi_approx = (3, 14, 159)
  let flags = (true, false, true)
  println(pi_approx)
  println(flags)
}

// Test array literals (if they should be lifted)
fn test_array_literals() -> Unit {
  let numbers = [1, 2, 3, 4, 5]
  println(numbers)
}

// Test generic function context (should not lift due to type params)
fn[T : Show] test_generic(value : T) -> T {
  let wrapper = (value, "wrapped")  // Should not lift due to generic context
  println(wrapper)
  value
}

// Test option lifting
fn test_option_lifting() -> Unit {
  let some_val = Some(42)
  let none_val : Option[Int] = None
  println(some_val)
  println(none_val)
}

// Test result lifting  
fn test_result_lifting() -> Unit {
  let ok_val : Result[Int, String] = Ok(100)
  let err_val : Result[Int, String] = Err("failed")
  println(ok_val)
  println(err_val)
}

// Test deeply nested structures
fn test_deep_nesting() -> Unit {
  let deep = (((1, 2), (3, 4)), ((5, 6), (7, 8)))
  println(deep)
}

// Test with different numeric types
fn test_numeric_types() -> Unit {
  let ints = (1, 2, 3)
  let doubles = (1.0, 2.0, 3.0) 
  let bytes : (Byte, Byte) = (0x41, 0x42)
  println(ints)
  println(doubles)
  println(bytes)
}

fn test_nested_tuple_strings() -> Unit {
  println(
    (("hello", "world"),
      ("foo", "bar"),
      ("baz", "qux"),
      ("corge", "grault"),
      ("garply", "waldo"),
      ("fred", "plugh"),
      ("xyzzy", "thud"),
      ("plugh", "grault"),
      ("grault", "corge"),
      ("waldo", "garply"),
      ("qux", "baz"),
      ("thud", "xyzzy"),
      ("plugh", "grault"),
      ("grault", "corge"),
      ("waldo", "garply")
      // ("qux", "baz"),
      // ("thud", "xyzzy"),
      // ("plugh", "grault"),
      // ("grault", "corge"),
      // ("waldo", "garply"),
      // ("qux", "baz"),
      // ("thud", "xyzzy"),
      // ("plugh", "grault"),
      // ("grault", "corge"),
      // ("waldo", "garply"),
      // ("qux", "baz"),
      // ("thud", "xyzzy"),
      // ("plugh", "grault"),
      // ("grault", "corge"),
      // ("waldo", "garply"),
      // ("qux", "baz"),
      // ("thud", "xyzzy")
    )
  )
}
// Main test function
fn init {
  test_basic_tuple()
  test_inline_tuple() 
  test_nested_tuple()
  test_string_constant()
  test_mixed_tuple()
  test_record_lifting()
  test_constructor_lifting()
  test_complex_structure()
  test_no_lift_runtime(5)
  test_no_lift_mutable()
  test_no_lift_empty()
  test_referencing_static()
  test_multiple_references()
  test_different_scopes()
  test_with_constants()
  test_array_literals()
  let _ = test_generic(42)
  test_option_lifting()
  test_result_lifting()
  test_deep_nesting()
  test_numeric_types()
  test_nested_tuple_strings()
  println("All static data lifting tests completed")
}
