struct View[T] {
  start: Int
  len: Int
  buf: FixedArray[T]
}

impl[X : Show] Show for View[X] with output(self, logger) {
  logger.write_string("[")
  for i = 0; i < self.len; i = i + 1 {
    if i > 0 {
      logger.write_string(", ")
    }
    self.buf[self.start + i].output(logger)
  }
  logger.write_char(']')
}

fn[T] View::length(self: View[T]) -> Int {
  self.len
}

fn[T] View::op_get(self: View[T], index: Int) -> T {
  self.buf[self.start + index]
}

struct MyFixedArray[T](FixedArray[T])

// TODO: bound check
// TODO: check op_as_view protocol
fn[T] MyFixedArray::op_as_view(self: MyFixedArray[T], start~: Int = 0, end?: Int) -> View[T] {
  let end = end.unwrap_or(self.length())
  View::{ start, len: end - start, buf: self.0 }
}

fn[T] MyFixedArray::length(self: MyFixedArray[T]) -> Int {
  self.0.length()
}

fn[T] MyFixedArray::make_view(self: MyFixedArray[T], start: Int, len: Int) -> View[T] {
  self[start:if start + len >= self.length() { self.length() } else { start + len }]
}

// TODO: bound check
fn[T] View::op_as_view(self: View[T], start~: Int = 0, end?: Int) -> View[T] {
  let end = end.unwrap_or(self.length())
  View::{ buf: self.buf, start: self.start+start, len: end-start }
}

fn init {
  let arr = MyFixedArray::MyFixedArray([1, 2, 3, 4, 5])
  let s1 = arr[1:]
  println(s1)
  let s2 = arr[:3]
  println(s2)
  let s3 = arr[1:3]
  println(s3)
  let s4 = arr[:]
  println(s4)
  let s5 = arr.make_view(1, 3)
  println(s5)
  let s6 = arr.make_view(3, 6)
  println(s6)
}

fn init {
  let arr = MyFixedArray::MyFixedArray([(1, 2), (3, 4), (5, 6), (7, 8), (9, 10)])
  let s1 = arr[1:][1:]
  println(s1)
  let s2 = arr[:3][:3]
  println(s2)
  let s3 = arr[:][1:3]
  println(s3)
}
