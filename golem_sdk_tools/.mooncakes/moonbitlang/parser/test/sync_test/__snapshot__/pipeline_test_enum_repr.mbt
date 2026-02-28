enum S {
  S0
  S1
  S2
  S3
  S4
}

pub fn f1(x: S) -> Unit {
  match x {
    S0 => println("s0")
    S1 => println("s1")
    S2 => println("s2")
    S3 => println("s3")
    S4 => println("s4")
  }
}

pub fn f2(x: S?) -> Unit {
  match x {
    Some(S0) => println("some s0")
    Some(S1) => println("some s1")
    None => println("none")
  }
}

fn init {
  f1(S0)
  f1(S1)
  f2(Some(S0))
}

enum T {
  T0
  T1(S)
  T2(S?)
}

fn test1(a: Int, b: Int) -> Unit {
  if (a == b) { println(".") } else { println("x") }
}

fn ft1(t: T) -> Int {
  match t {
    T0 => 0
    T1(S0) => 1
    T1(S1) => 2
    T2(None) => 3
    T2(Some(S0)) => 4
    T2(Some(S1)) => 5
  }
}

fn fs(os: S?) -> Int {
  match os {
    None => 0
    Some(S0) => 1
    Some(S1) => 2
  }
}

fn ft2(t: T) -> Int {
  match t {
    T2(os) => 3 + fs(os)
    T1(s) => {
      match s {
        S0 => 1
        S1 => 2
      }
    }
    T0 => 0
  }
}

fn fot1(ot: T?) -> Int {
  match ot {
    Some(t) => ft1(t) + 1
    None => 0
  }
}

fn fot2(ot: T?) -> Int {
  match ot {
    Some(t) => ft2(t) + 1
    None => 0
  }
}

fn init {
  test1(fot1(Some(T0)), 1)
  test1(fot1(Some(T1(S0))), 2)
  test1(fot1(Some(T1(S1))), 3)
  test1(fot1(Some(T2(None))), 4)
  test1(fot1(Some(T2(Some(S0)))), 5)
  test1(fot1(Some(T2(Some(S1)))), 6)
  test1(fot2(Some(T0)), 1)
  test1(fot2(Some(T1(S0))), 2)
  test1(fot2(Some(T1(S1))), 3)
  test1(fot2(Some(T2(None))), 4)
  test1(fot2(Some(T2(Some(S0)))), 5)
  test1(fot2(Some(T2(Some(S1)))), 6)
}
