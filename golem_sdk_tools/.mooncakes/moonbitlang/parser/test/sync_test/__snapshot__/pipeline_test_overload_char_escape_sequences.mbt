///|
fn main {
  println("--- Overloaded char literal for Byte ---")
  ('a' : Byte) |> println
  ('\xFF' : Byte) |> println
  ('\o377' : Byte) |> println
  ('\\' : Byte) |> println
  println("--- Overloaded char literal for Int ---")
  ('a' : Char) |> println
  ('\\' : Char) |> println
  ('\u{1F430}' : Char) |> println
  println("--- Array of overloaded char literal for Bytes ---")
  println((['a', '\xFF', '\o377', '\\'] : Bytes))
  println("--- Array of overloaded char literal for String ---")
  println((['a', '\\', '\u{1F430}'] : String))
}
 
