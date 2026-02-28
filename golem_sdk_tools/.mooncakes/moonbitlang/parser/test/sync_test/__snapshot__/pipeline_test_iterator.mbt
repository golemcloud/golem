
// an immutable iterator over integer
trait Iter {
  next(Self) -> (Int, Self)?
}

fn[I: Iter] for_(i: I, f: (Int) -> Unit) -> Unit {
  match i.next() {
    None => ()
    Some((elt, next)) => {
      f(elt)
      for_(next, f)
    }
  }
}


struct Range {
  beg: Int
  end: Int
}

fn range(beg: Int, end: Int) -> Range {
  { beg, end }
}

impl Iter for Range with next(self: Range) -> (Int, Range)? {
  if self.beg >= self.end {
    None
  } else {
    Some((self.beg, { ..self, beg: self.beg + 1 }))
  }
}


struct ArrayIter {
  arr: FixedArray[Int]
  idx: Int
}

impl Iter for ArrayIter with next(self: ArrayIter) -> (Int, ArrayIter)? {
  if self.idx >= self.arr.length() {
    None
  } else {
    Some((self.arr[self.idx], { ..self, idx: self.idx + 1 }))
  }
}


fn iter(arr: FixedArray[Int]) -> ArrayIter {
  { arr, idx: 0 }
}


fn init {
  for_(range(1, 10), fn (i) { println("\{i}\n") })
  println("====\n")
  for_(iter([1, 2, 3, 4, 5]), fn (i) { println("\{i}\n") })
}
