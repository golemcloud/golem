fn main {
  if match 1 { _ => true } {}
  if if true {1} else {2} { _ => () }
  if guard true {}
  if for i=0;i<10;i=i+1{} {}
  if try f catch {} {}
  if guard true {}
  if break 1 {}
  if return 1 { _ => () } 
  if continue 1 {}
}

fn f() -> Unit {
  match guard true {}
  if guard true {}
  guard guard true
  for i=0;i<10;i=if true {0} else {1} {}
}

fn g() -> Unit {
  if 1 + if true {} else {} == let a = b {
    _ => ()
  }
}

///|
fn common_sub_expression_in_arrow_function() -> Unit {
  if (x, y) let a = b {}
}