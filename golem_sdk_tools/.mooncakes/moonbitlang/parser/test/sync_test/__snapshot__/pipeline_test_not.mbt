fn main {
  let a = false
  let b = !a
  let b = !!b
  println(!b)
  if !a {
    println("a is false")
  } 
  println(f(true))
}

fn f(x : Bool) -> String {
  if !x {
    "false"
  } else {
    "true"
  }
} 


