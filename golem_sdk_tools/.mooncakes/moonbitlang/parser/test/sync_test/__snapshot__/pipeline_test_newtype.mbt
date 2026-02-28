struct MyInt(Int) derive(Show, Eq, Default, Compare)

fn init {
    let x = MyInt::MyInt(5)
    println(x)
    let y = MyInt::MyInt(6)
    println(y)
    let z = x.0 + (match y {
        MyInt(i) => i
    })
    println(z)
    let a = MyInt::MyInt(0)
    if a == MyInt::default() {
        println("ok")
    } else {
        println("not ok")
    }
    if x < y {
        println("ok")
    } else {
        println("not ok")
    }
}

struct WithInt[T]((Int, T)) derive(Show, Eq, Default, Compare)

fn init {
    let x = WithInt::WithInt((1, "one"))
    println(x)
    let y0 = (2, 2.0)
    let y = WithInt::WithInt(y0)
    println(y)
    let z1 = 
      x.0.0 +
      (match y {
        WithInt((i, _)) => i
      })
      
    println(z1)
    let z2 = 
      x.0.1 +
      (match y {
        WithInt((_, i)) => i.to_string()
      })

    println(z2)

    let a = WithInt::WithInt((0, 0))
    if a == WithInt::default() {
        println("ok")
    } else {
        println("not ok")
    }
    let y1 = WithInt::WithInt((2, 1.0))
    if y1 < y {
        println("ok")
    } else {
        println("not ok")
    }
}

enum MyEnum[T] {
    E1(WithInt[T])
    E2(MyInt)
} derive(Show)

struct E[T](MyEnum[T]) derive(Show)

fn init {
  let e1 = E::E(MyEnum::E1(WithInt::WithInt((1, "one"))));
  println(e1);
  let e2: E[Char] = E::E(MyEnum::E2(MyInt::MyInt(2)));
  println(e2);
}


struct MyStruct[T] {
    x: T
    y: S[T]
} derive(Show)

struct S[T](MyEnum[MyStruct[T]]) derive(Show)

fn init {
  let s0 = S::S(MyEnum::E2(MyInt::MyInt(2)));
  let s1 = S::S(MyEnum::E1(WithInt::WithInt((1, { x: "xxx", y: s0}))));
  println(s0)
  println(s1)
}
