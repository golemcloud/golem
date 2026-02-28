let global : Ref[Int] = { val: 42 }

fn init {
  let obj : &Show = global
  println(obj)
  println(global.val)
}
