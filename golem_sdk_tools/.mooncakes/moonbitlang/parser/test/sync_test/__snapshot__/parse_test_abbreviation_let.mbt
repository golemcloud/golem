
fn f(x : Bool) -> String{
  if x { "str1" } else { "str2" }
}

fn init{
    let _ = f(true) //discard return value
    let s = f(false)
}