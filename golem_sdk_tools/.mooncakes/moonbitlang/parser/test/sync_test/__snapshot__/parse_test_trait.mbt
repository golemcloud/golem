
/// doc for I
trait I {
  f1(Self) -> Int
  f2(Self, Int, Double) -> String
  f3() -> Self
}

trait Functor {
  fmap(Self) -> Self
}
