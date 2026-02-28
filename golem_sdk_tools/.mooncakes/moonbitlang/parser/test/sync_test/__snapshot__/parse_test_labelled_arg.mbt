
fn f(
  label~: Int,
  pos1,
  pos2,
  pos3
) {
}

fn f(x=default) {}

fn init {
  f(pos, pun~, label=expr)
  x.f(label=expr, pos, pun~)
  Constr(a, b)
  Constr(a, label~)
}

enum Labelled {
  A
  B(Int, x~ : String)
  C(x~ : Strint, Char, y~ : Bool)
}

fn f(x : Labelled) -> Unit {
  match x {
    A => ()
    B(x="1", y) => ()
    C(_, y~, x~) => ()
  }
}

// omit `~` when not pun
fn init {
  f(
    pun, // variable, positional
    pun~, // labelled, pun
    label=expr // ok
  )
  Constr(l1=e1, l2=e2)
  match x {
    B(x="1", y~, z) => ()
  }
}

fn init {
  f(x~=1, y=2)
  match x {
    B(x~=_) => ()
  }
}
