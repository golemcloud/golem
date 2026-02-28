///|
fn BytesView::len(self : BytesView) -> Int = "%bytesview.len"

///|
pub fn f(a : BytesView) -> Unit {
  let x = {
    let b = a
    println(b)
    b
  }.len()
  ignore(x)
}

///|
fn main {
  f([1, 2, 3])
}
