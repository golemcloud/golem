
trait X : Show {
  name (Self) -> String
}

struct Stock(String) derive(Show)

impl X for Stock with name( self : Stock) -> String {
  self.0
}


fn init {
  let v = Stock::Stock("x") as &X
  println(v)
}
