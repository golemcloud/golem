enum E {
  A = 10
  B
  C
  D = 0
  E
  F
  G = 0xff
}

fn E::unsafe_as_int(self : E) -> Int = "%identity"

fn E::as_int(self : E) -> Int {
  match self {
    A => 10
    B => 11
    C => 12
    D => 0
    E => 1
    F => 2
    G => 255
  }
}

fn init {
  for x in [ A, B, C, D, E, F, G ] {
    guard x.as_int() == x.unsafe_as_int() else { panic() }
  }
}
