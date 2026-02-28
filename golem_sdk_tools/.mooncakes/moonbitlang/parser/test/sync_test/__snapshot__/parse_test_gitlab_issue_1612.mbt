/// for more information, see MR !4034 on GitLab
fn div(x : Int,y : Int) -> Int raise {
  if y == 0 {
    raise (Failure "x") // parse error
  }
  x / y 
}

fn hello(x : Int,y : Int) -> Int raise {
   div(x,y)
}
