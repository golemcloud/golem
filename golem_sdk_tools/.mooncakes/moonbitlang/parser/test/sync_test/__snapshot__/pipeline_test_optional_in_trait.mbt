trait I {
  f(Self, x~ : Int, opt? : Int) -> Unit = _
}

impl I with f(_self, x~, opt? = x + 1) {
  println(opt)
}

struct T1 (Int)
struct T2 (Int)

impl I for T1 with f(_self, x~, opt?) {
  ignore(x)
  println(opt)
}

impl I for T2

fn main {
  I::f(T1(0), x=1, opt=42)
  I::f(T1(0), x=1)
  I::f(T2(0), x=1, opt=42)
  I::f(T2(0), x=1)
  let obj1 = T1(0) as &I
  obj1.f(x=1, opt=42)
  obj1.f(x=1)
  let obj2 = T2(0) as &I
  obj2.f(x=1, opt=42)
  obj2.f(x=1)
}
