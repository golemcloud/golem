trait Set {
  has(Self, Int) -> Bool
}

struct FnSet((Int) -> Bool)
impl Set for FnSet with has(self, k) { (self.0)(k) }

fn &Set::empty() -> &Set {
  FnSet(_x => false)
}

fn &Set::add(self : &Set, new_k : Int) -> &Set {
  FnSet(k => k == new_k || self.has(k))
}

fn init {
  let s =
    &Set::empty()
    |> &Set::add(1)
    |> &Set::add(2)
    |> &Set::add(3)
  println(s.has(42))
}
