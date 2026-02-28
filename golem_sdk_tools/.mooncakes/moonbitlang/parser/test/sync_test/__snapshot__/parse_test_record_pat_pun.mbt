struct R {
  r1:String
  r2:Int
  r3:Bool
}

fn init {
  let aa = { r1:"a", r2:1, r3:true }
  let {r1, r2, r3} = aa
  println(r1)
}

