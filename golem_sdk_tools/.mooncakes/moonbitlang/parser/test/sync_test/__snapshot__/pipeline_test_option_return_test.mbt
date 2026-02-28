struct Vector[X] {
  mut data : FixedArray[X]
  mut len : Int
}

/// `length[X](self : Vector[X])`
///
/// Get length of a vector
fn[X] Vector::length(self : Vector[X]) -> Int {
  self.len
}

/// `op_get[X](self : Vector[X], i : Int)`
///
/// Get the ith element of vector
fn[X] Vector::op_get(self : Vector[X], i : Int) -> X? {
  if i < self.len {
    Some(self.data[i])
  } else {
    None
  }
}

/// `to_string[X : Show](self : Vector[X]) -> String`
///
/// convert a vector to string, type inside the vector must have method `to_string`
impl[T : Show] Show for Vector[T] with output(self, logger) {
  // let mut str = "["
  logger.write_string("[")
  for i = 0; i < self.length(); i = i + 1 {
    self[i].unwrap().output(logger)
    logger.write_char(',')
  }
  logger.write_string("]")
}

fn init {
  println({ data: [1, 2, 3], len: 2 })
}
