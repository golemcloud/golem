
fn T::method_name(x: Int) {}
fn @pkg.T::method_name() {}

fn init {
  T::method_name(1)
  @pkg.T::method_name()
}

priv type T
pub fn T::f() {}
