
// count the number of "1"s in a 32-bit integer
fn ctpop(n: Int) -> Int {
  // extract the lower bit of every 2-bit group
  let low_of_2bits = 0x55555555
  // count the number of "1"s in every 2-bit group
  // store the result in the same 2-bit group
  let c2 = (n & low_of_2bits) + ((n.reinterpret_as_uint() >> 1).reinterpret_as_int() & (low_of_2bits))
  // count the number of "1"s in every 4-bit group (i.e. two 2-bit groups),
  // store the result in the same 4-bit group
  let low_of_4bits = 0x33333333
  let c4 = c2.land(low_of_4bits) + ((c2.reinterpret_as_uint() >> 2).reinterpret_as_int() & low_of_4bits)
  // similarly for every 8-bit group
  let low_of_8bits = 0x0f0f0f0f
  let c8 = c4.land(low_of_8bits) + ((c4.reinterpret_as_uint() >> 4).reinterpret_as_int() & low_of_8bits)
  // 16-bit group.
  // there will be only two groups left, without rist of overlapping,
  // so there's no need to clean the unused higher bits
  let c16 = c8 + (c8.reinterpret_as_uint() >> 8).reinterpret_as_int()
  // the final 32-bit group
  // [ctpop(x) <= 32], so only the last 6 bits are meaningful
  let lower_6bits = 63 // 0x3f
  (c16 + (c16.reinterpret_as_uint() >> 16).reinterpret_as_int()).land(lower_6bits)
}


// a simple bit set to store a set of integers less than 32
struct Bitset(Int)

let empty_bitset: Bitset = Bitset(0)

fn Bitset::has(self: Bitset, idx: Int) -> Bool {
  self.0.land((1 << idx)) != 0
}

fn Bitset::index_of(self: Bitset, idx: Int) -> Int {
  let items_below_idx = self.0.land((1 << idx) - 1)
  ctpop(items_below_idx)
}

fn Bitset::add(self: Bitset, idx: Int) -> Bitset {
  Bitset(self.0.lor((1 << idx)))
}

fn Bitset::remove(self: Bitset, idx: Int) -> Bitset {
  Bitset(self.0.lxor((1 << idx)))
}


// A sparse array with at most 32 elements, where elements are not required to have contiguous index.
// Empty elements don't waste any space, without losing constant-time access
struct Sparse_Array[X] {
  // record which elements are present
  elem_info: Bitset
  data: FixedArray[X]
}

fn[X] make_sparse_array(idx: Int, value: X) -> Sparse_Array[X] {
  { elem_info: empty_bitset.add(idx), data: [ value ] }
}

fn[X] Sparse_Array::has(self: Sparse_Array[X], idx: Int) -> Bool {
  self.elem_info.has(idx)
}

fn[X] Sparse_Array::op_get(self: Sparse_Array[X], idx: Int) -> X? {
  if self.elem_info.has(idx) {
    Some (self.data[self.elem_info.index_of(idx)])
  } else {
    None
  }
}

// add a new element into the sparse array.
// [idx] must be absent from [self]
fn[X] Sparse_Array::add(self: Sparse_Array[X], idx: Int, value: X) -> Sparse_Array[X] {
  let new_len = self.data.length() + 1
  let pos_of_new_item = self.elem_info.index_of(idx)
  let new_data = FixedArray::make(new_len, value)
  let mut i = 0
  while i < pos_of_new_item {
    new_data[i] = self.data[i]
    i = i + 1
  }
  new_data[i] = value
  i = i + 1
  while i < new_len {
    new_data[i] = self.data[i - 1]
    i = i + 1
  }
  { elem_info: self.elem_info.add(idx), data: new_data }
}

// replace an existing element in the sparse array.
fn[X] Sparse_Array::replace(self: Sparse_Array[X], idx: Int, value: X) -> Sparse_Array[X] {
  let len = self.data.length()
  let pos_of_new_item = self.elem_info.index_of(idx)
  let new_data = FixedArray::make(len, value)
  for i = 0; i < len; i = i + 1 {
    new_data[i] = if i == pos_of_new_item { value } else { self.data[i] }
  }
  { elem_info: self.elem_info, data: new_data }
}


// Hash-Array-Mapped-Trie (HAMT) is a persistent hash-table data structure.
// It is a trie over the hash of keys (i.e. strings of binary digits)
//
// Every level in a HAMT can have up to 32 branches (5 digits),
// so HAMT has a tree height of at most 7,
// and is more efficient compared to most other tree data structures.
//
// HAMT uses the bitmap-based sparse array above to avoid space waste
enum HAMT[K, V] {
  Empty
  Leaf(K, V) // optimize for the case of no collision
  Collision(Bucket[K, V]) // use a list of buckets to resolve collision
  Branch(Sparse_Array[HAMT[K, V]])
}

enum Bucket[K, V] {
  Just_One(K, V) // must be non-empty
  More(K, V, Bucket[K, V])
}

// The number of bits consumed at every [Branch] node
let segment_length: Int = 5

fn[K, V] make() -> HAMT[K, V] {
  Empty
}

fn[K: Eq, V] HAMT::find_with_hash(self: HAMT[K, V], key: K, hash: Int) -> V? {
  match self {
    Empty => None
    Leaf(key1, value) => {
      if key == key1 { Some(value) } else { None }
    }
    Collision(bucket) => bucket.find(key)
    Branch(children) => {
      // get the first segment (lower 5 bits) of the hash value
      let idx = hash.land((1 << segment_length) - 1)
      match children[idx] {
        Some(child) =>
          // when searching recursively, drop the segment just used
          child.find_with_hash(key, (hash.reinterpret_as_uint() >> segment_length).reinterpret_as_int())
        None => None
      }
    }
  }
}

fn[K: Eq + Hash, V] HAMT::find(self: HAMT[K, V], key: K) -> V? {
  self.find_with_hash(key, key.hash())
}

fn[K: Eq, V] HAMT::add_with_hash(
  self: HAMT[K, V],
  key: K,
  depth: Int,
  hash: Int,
  value: V) -> HAMT[K, V] {
  // make sure leaf nodes always appear at the bottom of the tree
  fn make_leaf(depth: Int, key: K, hash: Int, value: V) {
    if (depth >= 32) {
      HAMT::Leaf(key, value)
    } else {
      let idx = hash.land((1 << segment_length) - 1)
      let child = make_leaf(depth + segment_length, key, (hash.reinterpret_as_uint() >> segment_length).reinterpret_as_int(), value)
      HAMT::Branch(make_sparse_array(idx, child))
    }
  }
  match self {
    Empty => make_leaf(depth, key, hash, value)
    Leaf(key1, value1) => {
      if key == key1 {
        Leaf(key, value)
      } else {
        Collision(More(key, value, Just_One(key1, value1)))
      }
    }
    Collision(bucket) => {
      Collision(bucket.add(key, value))
    }
    Branch(children) => {
      let idx = hash.land((1 << segment_length) - 1)
      match children[idx] {
        Some(child) => {
          let child = child.add_with_hash(key, depth + segment_length, (hash.reinterpret_as_uint() >> segment_length).reinterpret_as_int(), value)
          Branch(children.replace(idx, child))
        }
        None => {
          let child = make_leaf(depth + segment_length, key, (hash.reinterpret_as_uint() >> segment_length).reinterpret_as_int(), value)
          Branch(children.add(idx, child))
        }
      }
    }
  }
}

fn[K: Eq + Hash, V] HAMT::add(self: HAMT[K, V], key: K, value: V) -> HAMT[K, V] {
  self.add_with_hash(key, 0, key.hash(), value)
}


fn[K, V] HAMT::size(self: HAMT[K, V]) -> Int {
  match self {
    Empty => 0
    Leaf(_) => 1
    Collision(bucket) => bucket.size()
    Branch(children) => {
      let mut total_size = 0
      for i = 0; i < children.data.length(); i = i + 1 {
        total_size = total_size + children.data[i].size()
      }
      total_size
    }
  }
}

fn[K, V] HAMT::count_collision(self: HAMT[K, V]) -> Int {
  match self {
    Empty | Leaf(_) => 0
    Collision(bucket) => bucket.size()
    Branch(children) => {
      let mut total_count = 0
      for i = 0; i < children.data.length(); i = i + 1 {
        total_count = total_count + children.data[i].count_collision()
      }
      total_count
    }
  }
}

fn[K: Show, V: Show] print_hamt(map : HAMT[K, V], indent: Int) -> Unit {
  match map {
    Empty => println("Empty\n")
    Leaf(k, v) => println("\{k} => \{v}\n")
    Collision(bucket) => print_bucket(bucket)
    Branch(children) =>
      if (children.data.length() == 0) {
        println("Empty\n")
      } else {
        println("Branch\n")
        for i = 0; i < 32; i = i + 1 {
          match children[i] {
            Some(child) => {
              for n = 0; n < indent; n = n + 1 {
                println(' ')
              }
              println("#\{i}\n")
              for n = 0; n < indent; n = n + 1 {
                println(' ')
              }
              println("- ")
              print_hamt(child, indent + 2)
            }
            None => ()
        }
      }
    }
  }
}


// bucket operations
fn[K: Eq, V] Bucket::find(self: Bucket[K, V], key: K) -> V? {
  match self {
    Just_One(key1, value) => {
      if key == key1 { Some(value) } else { None }
    }
    More(key1, value, rest) => {
      if key == key1 { Some(value) } else { rest.find(key) }
    }
  }
}

fn[K: Eq, V] Bucket::add(self: Bucket[K, V], key: K, value: V) -> Bucket[K, V] {
  match self {
    Just_One(key1, value1) => {
      if key == key1 {
        Just_One(key, value)
      } else {
        More(key, value, self)
      }
    }
    More(key1, value1, rest) => {
      if key == key1 {
        More(key, value, rest)
      } else {
        More(key1, value1, rest.add(key, value))
      }
    }
  }
}

fn[K, V] Bucket::size(self: Bucket[K, V]) -> Int {
  match self {
    Just_One(_) => 1
    More(_, _, rest) => rest.size() + 1
  }
}

fn[K: Show, V: Show] print_bucket(bucket : Bucket[K, V]) -> Unit {
  match bucket {
    Just_One(k, v) => println("\{k} => \{v}\n")
    More(k, v, rest) => {
      println("\{k} => \{v}; ")
      print_bucket(rest)
    }
  }
}


fn init {
    let n = 10
    let mut hamt = make()
    for i = 0; i < n; i = i + 1 {
      hamt = hamt.add(i.to_string(), i)
    }

    let collision_count = hamt.count_collision()
    println("number of collision: \{collision_count}\n")
    // uncomment this to see the tree (large trees may not display properly)
    // hamt.println(0)

    let mut is_ok = true
    for i = 0; i < n; i = i + 1 {
      match hamt.find(i.to_string()) {
        Some(v) =>
          if v != i {
            is_ok = false
            println("\{i}: wrong value\n")
          }
        None => {
          is_ok = false
          println("\{i}: not found\n")
        }
      }
    }

    if (is_ok) {
      println("OK!\n")
    }
}
