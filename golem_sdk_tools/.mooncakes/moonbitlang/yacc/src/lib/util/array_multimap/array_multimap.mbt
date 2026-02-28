// Copyright (C) 2025 International Digital Economy Academy
//
// This program is free software; you can redistribute it and/or
// modify it under the terms of the GNU General Public License
// as published by the Free Software Foundation; version 2.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program; if not, see <https://www.gnu.org/licenses/>.

///|
struct T[K, V](@hashmap2.T[K, Array[V]])

///|
pub fn[K, V] new() -> T[K, V] {
  @hashmap2.new(capacity=16)
}

///|
pub fn[K : Hash + Eq, V : Eq] T::add(
  self : T[K, V],
  key : K,
  value : V,
) -> Unit {
  let arr = self.0.get_or_init(key, fn(_) { [] })
  if not(arr.contains(value)) {
    arr.push(value)
  }
}

///|
pub fn[K : Hash + Eq, V] T::get(self : T[K, V], key : K) -> Array[V] {
  self.0.get(key).unwrap_or([])
}

///|
pub impl[K : Eq + Hash + Show, V : ToJson] ToJson for T[K, V] with to_json(self) {
  Map::from_iter(self.0.iter()).to_json()
}

///|
pub fn[K : Hash + Eq, V] T::copy(self : T[K, V]) -> T[K, V] {
  let map2 = @hashmap2.new(capacity=16)
  for entry in self.0.iter() {
    map2.set(entry.0, entry.1)
  }
  T(map2)
}
