trait I {
  add(Self, Self) -> Self
}

struct T(Int) derive(Show)
impl I for T with add(self : T, other : T) -> T = "%i32_add"

fn init {
  I::add(T::T(2), T::T(3)) |> println
}
