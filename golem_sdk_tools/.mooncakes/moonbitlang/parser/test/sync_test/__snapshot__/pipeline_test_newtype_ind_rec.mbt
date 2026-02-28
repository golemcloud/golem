struct T1(T2)
struct T2(T1)

struct T3(T4)

enum T4 {
  Nil
  Cons(Int,T3)
}

fn length(x : T3) -> Int{
  match x.0 {
    Nil => 0
    Cons(_,y)=>length(y)
  }
}

fn init {
  println(length(T3(Nil)))
  println(length(T3(Cons(3,T3(Nil)))))
}
