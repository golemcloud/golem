fn[X] id(x : X)->(X,X){
  (x,x)
}
fn init {
  let v = []
  // let id = id
  let v0 = {
    println(v)
    id(v)
  }
  let u = [3].append(v0.0)
  println(v)
}