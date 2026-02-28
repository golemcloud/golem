// 1. contify expose new stack alloc opurtunity
pub fn f1() -> Int {
  let mut x = 1
  fn aux() {
    x = x + 1
    x
  }
  aux()
}

// 2. inlining/intrinsic expose new stack alloc oppurtunity
pub fn f2() -> Int {
  let mut sum = 0
  ([ 1, 2, 3 ]: FixedArray[_]).each(x => { sum = sum + x })
  sum
}

// 3. stack alloc opt for [Ref]
fn f3() -> Int {
  let r = Ref::{ val: 1 }
  r.val = r.val + 1
  r.val
}

// 4. stack alloc opt for structs with multiple fields
struct S {
  x : Int
  mut y : Int
}

pub fn f4() -> Int {
  let s = S::{ x: 2, y: 3 }
  s.y = s.x + 1
  s.x * s.y
}

// 5. interaction with match
pub fn f5() -> Int {
  let s = S::{ x: 2, y: 3 }
  match s {
    { x: 2, .. } => s.y = s.y + 1
    _ => ()
  }
  s.x * s.y
}

fn init {
  println(f1())
  println(f2())
  println(f3())
  println(f4())
  println(f5())
}
