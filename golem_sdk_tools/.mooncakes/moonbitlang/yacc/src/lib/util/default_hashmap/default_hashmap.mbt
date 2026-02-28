///|
pub type T[K, V] = DefaultHashMap[K, V]

///|
struct DefaultHashMap[K, V] {
  map : @hashmap2.T[K, V]
  default_fn : (K) -> V
}

///|
pub fn[K, V] new(default_fn : (K) -> V) -> T[K, V] {
  DefaultHashMap::{ map: @hashmap2.new(capacity=16), default_fn }
}

///|
pub fn[K : Eq + Hash, V] T::get(self : T[K, V], key : K) -> V {
  self.map.get_or_init(key, fn(key) { (self.default_fn)(key) })
}
