
struct Self {}
enum Self {}

fn[Self] f(x: Self) -> Self { x }

priv struct T {}

pub trait I {
  f(Self) -> T
  f[X](Self) -> Int
}

trait I_private { m(Self) }

pub fn[X: I_private] f(x: X) {}

trait I_dup_method {
  m1(Self)
  m1(Self)
  m2(Self) -> Int
  m2(Self) -> Double
  m2(Self) -> Char
}

pub fn I_private::m(self: Int) {}
fn I_private::m(self: String) {}
