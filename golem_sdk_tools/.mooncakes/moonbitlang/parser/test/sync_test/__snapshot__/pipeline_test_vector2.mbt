
struct Vector[X] {
  mut data: UninitializedArray[X]
  mut len: Int
}

fn[X] Vector::length(self: Vector[X]) -> Int {
  self.len
}

fn[X] Vector::capacity(self: Vector[X]) -> Int {
  self.data.length()
}


fn[X] Vector::new(cap: Int) -> Vector[X] {
  { data: UninitializedArray::make(cap), len: 0 }
}


fn[X] Vector::op_get(self: Vector[X], i: Int) -> X? {
  if i < self.len { Some(self.data[i]) } else { None }
}


fn[X] Vector::push_back(self: Vector[X], elem: X) -> Unit {
  if self.len < self.data.length() {
    self.data[self.len] = elem
    self.len = self.len + 1
  } else {
    let old_cap = self.data.length()
    let new_cap = if old_cap == 0 { 10 } else { old_cap * 2 }
    let new_data = UninitializedArray::make(self.data.length() * 2)
    for i = 0; i < self.len; i = i + 1 {
      new_data[i] = self.data[i]
    }
    new_data[self.len] = elem
    self.data = new_data
    self.len = self.len + 1
  }
}


struct T(Int) derive (Default)
struct U(Int)

fn init {
  let v1: Vector[T] = Vector::new(2)
  v1.push_back(T(0))
  v1.push_back(T(1))
  v1.push_back(T(2))
  v1.push_back(T(3))
  for i = 0; i < v1.length(); i = i + 1 {
    match v1[i] {
      Some(T(t)) => println(t)
      None => ()
    }
  }


  let v2: Vector[U] = Vector::new(2)
  v2.push_back(U(0))
  v2.push_back(U(1))
  v2.push_back(U(2))
  v2.push_back(U(3))
  for i = 0; i < v2.length(); i = i + 1 {
    match v2[i] {
      Some(U(u)) => println(u)
      None => ()
    }
  }


  let v3 = Vector::new(2)
  v3.push_back(0.1)
  v3.push_back(1.1)
  v3.push_back(2.1)
  v3.push_back(3.1)
  for i = 0; i < v3.length(); i = i + 1 {
    match v3[i] {
      Some(x) => println(x)
      None => ()
    }
  }

}
