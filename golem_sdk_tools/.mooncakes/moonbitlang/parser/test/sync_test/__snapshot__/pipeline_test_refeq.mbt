fn[T] xtest_refeq(x: T, y: T) -> Unit {
  if physical_equal(x,  y) {
    println(".")
  } else {
    println("x")
  }
}

struct S {
  x: Int
  mut y: Int
}

fn f1(x: Int) -> Int {
  x
}

fn f2(x: Int) -> Int {
  x
}

fn init {
  xtest_refeq(1, 1);
  xtest_refeq("xxx", "xxx");
  let a = FixedArray::make(3, 'c')
  let b = a
  xtest_refeq(a, b);
  let s1 = { x: 1, y: 2 }
  let s2 = s1
  s1.y = 3
  xtest_refeq(s1, s2);
  xtest_refeq(f1, f1);
  xtest_refeq((Option::None: Int?), Option::None)

}

fn init {
  let a = FixedArray::make(3, 'c')
  let c = FixedArray::make(3, 'c')
  xtest_refeq(1, 2);
  xtest_refeq(a, c);
  let s1 = { x: 1, y: 2 }
  let s2 = { ..s1, y: 3 }
  xtest_refeq(s1, s2);
  xtest_refeq(f1, f2);
  xtest_refeq(Option::Some(1), Option::Some(1))
}
