// basic
struct R[X] {
  x : X
  y : X
}

// toplevel should not be RC'ed
let top_r : R[Int] = { x: 1, y: 2 }

fn[X, Y] R::combine(self : R[X], other : R[Y]) -> R[(X, Y)] {
  { x: (self.x, other.x), y: (self.y, other.y) }
}

pub fn basic1() -> R[(Int, String)] {
  let r1 = R::{ x: 1, y: 2 }
  let _ = r1.combine(r1)
  let r2 = R::{ x: "a", y: "b" }
  r1.combine(r2)
}

// join point
pub fn f_join1(x : Option[Bool]) -> Unit {
  let r = R::{ x: 1, y: 2 }
  match x {
    Some(true) | None =>  ignore(r)
    Some(false) => ()
  }
}

pub fn f_join2(r : R[Int], b : Bool) -> Unit {
  if b {
    return
  }
  let _ = r
}

pub fn f_loop1(r1 : R[Int], r2 : R[Int], r3 : R[Int], c : Int) -> Unit {
  loop (c, r1) {
    (0, _) => ignore(r2)
    (1, _) => return
    (n, r) => continue (n - 2, r)
  }
  let _ = r3
}


// branching
pub fn br1(x : Option[R[Int]]) -> R[Int] {
  match x {
    Some(r) => r
    None => top_r
  }
}

pub fn br2(r1 : R[Int], r2 : R[Int], r3 : R[Int], x : Int) -> R[Int] {
  match x {
    0 => r1
    1 => r2
    _ => top_r
  }
}

enum E {
  C1
  C2(R[Int], R[Int])
}

pub fn br3(e : E) -> Unit {
  match e {
    C1 => ()
    C2(x, y) => ignore(x.combine(y))
  }
}

pub fn br4(e : E) -> R[Int] {
  match e {
    C1 => top_r
    C2(x, _) => x
  }
}

pub fn br5(e : E) -> Unit {
  match e {
    C1 => ()
    C2(_) => ()
  }
}

// eliminators
pub fn elim1(b : Bytes, s : String) -> Int {
  b.length() + s.length()
}

// assignment
pub fn assign1() -> Unit {
  let mut r = R::{ x: 1, y: 2 }
  r = R::{ x: 2, y: 1 }
  r = R::{ x: r.y, y: r.x }
}

struct M[X] {
  mut m : X
}

pub fn mutate1(m : M[R[Int]]) -> Unit {
  m.m = top_r
}

pub fn mutate2(m : M[R[Int]]) -> Unit {
  m.m = { ..m.m, y: 3 }
}

pub fn mutate3(xs : FixedArray[R[Int]]) -> Unit {
  xs[0] = xs[1]
}

pub fn mutate4(xs : FixedArray[R[Int]]) -> FixedArray[R[Int]] {
  xs[0] = xs[1]
  xs
}

// misc
fn _refeq() -> Unit {
  let x = R::{ x: 1, y: 2 }
  let y = R::{ x: 1, y: 2 }
  let _ = physical_equal(x, y)
  let _ = physical_equal(y, y)
}
