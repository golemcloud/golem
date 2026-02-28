
impl Eq for Int with op_equal(x, y) {}
impl[X: Eq] Eq for Array[X] with op_equal(xs, ys) {}
impl Eq with op_notequal(x: Self, y: Self) -> Bool {}

// error recovery
impl[X: Eq] Eq for Array[X with op_equal(xs, ys) {}

/// docstring
impl Hash for Int with hash(self) -> Int {}


fn error_here()
impl Trait with method() {}

fn error_here()
impl Trait for Int with method() {}

impl[X] Trait with method() {}

// error
impl[X] Trait with method

impl Eq for Int with op_equal(x, y) = "%i32.eq"

#deprecated("msg")
impl Eq for Int
