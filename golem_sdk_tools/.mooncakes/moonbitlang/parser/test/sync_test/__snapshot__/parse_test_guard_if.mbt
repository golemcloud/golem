fn f1(x : Int) -> Unit {
  match x {
    x if if x > 0 { true } else { false } => println(x)
    _ => println(x)
  }
}

fn f2(x : Int) -> Unit {
  match x {
    x if (if x > 0 { true } else { false }) => println(x)
    _ => println(x)
  }
}

fn g1(x : Int) -> Unit {
  match x {
     x if match x { _ => true}  => println(x)
    _ => println(x)
  }
}

fn g2(x : Int) -> Unit {
  match x {
     x if (match x { _ => true})  => println(x)
    _ => println(x)
  }
}