struct Map[K, V](FixedArray[(K, V)])

pub fn[K : Eq, V] Map::find(self: Map[K, V], key: K) -> V? {
  let mut i = 0
  while i < self.0.length() {
    let (k, v) = self.0[i]
    if (key == k) {
        return Some(v)
    } else {
        i = i + 1
    }
  }
  return None
}

pub(all) enum T {
  A
  B
  C
} derive (Eq)

struct ST(T) derive (Eq)

fn init {
    let map1: Map[T, _] = Map([(A, "ok\n"), (B, "not ok\n"), (C, "not ok\n")])
    let map2 = Map::Map([("a", 0), ("b", 1), ("c", 2)] )
    let map3: Map[ST, _] = Map([(ST(A), 1.28), (ST(B), 2.56), (ST(C), 3.14)])
    println(map1.find(A))
    println(map2.find("b"))
    println(map3.find(ST(C)))
}
