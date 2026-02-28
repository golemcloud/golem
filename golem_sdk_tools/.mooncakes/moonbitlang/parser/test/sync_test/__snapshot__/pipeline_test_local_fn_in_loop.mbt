///|
#locals(h)
pub fn f(h : (Int) -> Unit) -> Unit {
  h(1)
}

///|
#locals(h)
pub fn f1(h : (Int) -> Unit) -> Unit {
  while true {
    h(1)
    break
  }
}

///|
#locals(h)
pub fn f2(h : (Int) -> Unit) -> Unit {
  for i in 1..=10 {
    h(i)
  }
}

///|
#locals(h)
pub fn f3(h : (Int) -> Unit) -> Unit {
  for i = 0; i < 10; i = i + 1 {
    h(i)
  }
}

///|
#locals(h)
pub fn f4(h : (Int) -> Unit) -> Unit {
  loop 1 {
    a if a > 5 => h(a)
    a => continue a + 1
  }
}

///|
#locals(h)
pub fn f5(h : (Int) -> Unit) -> Unit {
  loop 1 {
    a if a > 5 => h(a)
    a => continue a + 1
  }
  h(42)
}

///|
#locals(h)
pub fn f6(h : (Int) -> Unit) -> Unit {
  loop 1 {
    a if a > 5 => {
      h(a)
      h(a)
    }
    a => continue a + 1
  }
}

///|
fn main {
  f(a => println(a))
  f1(a => println(a))
  f2(a => println(a))
  f3(a => println(a))
  f4(a => println(a))
  f5(a => println(a))
  f6(a => println(a))
}
