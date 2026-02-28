
/// this file is wrongly parsed 
/// as several toplevel let phrases
fn main {
  // Define an integer variable with a value of 10.
  match c {
    1 =>
      let x = 3
  }
  
  // We also refer to an immutable variable as a binding to the value 10 with the type Int.  
  // Define a new binding named `b` and shadow the previous `b`.  
  let b = false
  println(b)
}