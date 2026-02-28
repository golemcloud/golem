fn init {
  let r1 : Ref[_] = { val: 1 }
  let r2 : Ref[_] = { val: 2 }
  let mut r = r1
  r.val += { r = r2; 1 }
  println((r1, r2))
}

fn init {
  let r1 : Ref[_] = { val: 1 }
  let r2 : Ref[_] = { val: 2 }
  let mut r = r1
  r.val = r.val + { r = r2; 1 }
  println((r1, r2))
}

fn init {
  let r1 : Ref[_] = { val: 1 }
  let r2 : Ref[_] = { val: 2 }
  let mut r = r1
  let obj = r
  obj.val = obj.val + { r = r2; 1 }
  println((r1, r2))
}


struct MyInt(Int) derive(Show)
let bad_global : Ref[() -> Unit] = { val: fn () {} }
impl Add for MyInt with add(self : MyInt, other : MyInt) -> MyInt {
  (bad_global.val)()
  MyInt(self.0 + other.0)
}

fn init {
  let r1 : Ref[_] = { val: MyInt::MyInt(1) }
  let r2 : Ref[_] = { val: MyInt::MyInt(2) }
  let mut r = r1
  bad_global.val = fn () { r = r2 }
  r.val += MyInt::MyInt(1)
  println((r1, r2))
}

fn init {
  let r : Ref[_] = { val: 1 }
  r.val += { r.val = 42; r.val }
  println(r)
}
