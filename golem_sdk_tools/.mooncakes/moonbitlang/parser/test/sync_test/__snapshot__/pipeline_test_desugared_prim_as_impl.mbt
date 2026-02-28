trait Range {
  start(Self) -> Int
  len(Self) -> Int
}

fn[R : Range] print_range(r : R) -> Unit {
  println("\{r.start()}-\{r.start() + r.len()}")
}

impl Range for BytesView with start(self) = "%bytesview.start"
impl Range for BytesView with len(self) = "%bytesview.len"

fn main {
  print_range(b"abcd"[:])
  print_range(b"abcd"[1:3])
}
