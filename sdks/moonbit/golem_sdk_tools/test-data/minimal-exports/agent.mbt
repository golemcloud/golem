///|
#derive.agent
struct Counter {
  mut value : Int
} derive(ToJson, FromJson)

///|
fn Counter::new() -> Counter {
  { value: 7, }
}

///|
pub fn Counter::add(self : Self, amount : Int) -> Int {
  self.value += amount
  self.value
}
