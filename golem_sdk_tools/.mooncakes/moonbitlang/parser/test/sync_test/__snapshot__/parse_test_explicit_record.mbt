
fn init {
  let _ = T::{ x: 1, y: 2 }
  let _ = T::{ x } // pun
  let _ = T::{ x }
  let _ = @pkg.T :: { x, y }
  let _ = @pkg.T :: { ..r, x }
  let _ = @pkg.T :: { ..r }
}
