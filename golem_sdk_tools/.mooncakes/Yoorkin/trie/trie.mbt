///|
struct Trie[A] {
  value : A?
  forks : @immut/sorted_map.SortedMap[Char, Trie[A]]
}

///|
pub typealias Trie as T

///|
pub fn[A] Trie::lookup(self : Self[A], path : String) -> A? {
  loop (path.view(), self) {
    ([], trie) => trie.value
    ([x, .. xs], trie) =>
      match trie.forks.get(x) {
        None => None
        Some(subtree) => continue (xs, subtree)
      }
  }
}

///|
#deprecated("use `add` instead")
pub fn[A] Trie::insert(self : Self[A], path : String, value : A) -> Trie[A] {
  self.add(path, value)
}

///|
pub fn[A] Trie::add(self : Self[A], path : String, value : A) -> Trie[A] {
  fn aux(xs, trie) {
    match xs {
      ([] : @string.View) => Trie::{ ..trie, value: Some(value) }
      [x, .. xs] => {
        let subtree = trie.forks
          .get(x)
          .unwrap_or_else(() => { value: None, forks: @immut/sorted_map.new() })
        { ..trie, forks: trie.forks.add(x, aux(xs, subtree)) }
      }
    }
  }

  aux(path.view(), self)
}

///|
#deprecated("Use Trie::empty instead")
pub fn[A] empty() -> Trie[A] {
  Trie::{ value: None, forks: @immut/sorted_map.new() }
}

///|
pub fn[A] Trie::empty() -> Trie[A] {
  Trie::{ value: None, forks: @immut/sorted_map.new() }
}

///|
pub fn[A] Trie::of(data : FixedArray[(String, A)]) -> Trie[A] {
  loop (0, Trie::empty()) {
    (i, trie) =>
      if i == data.length() {
        trie
      } else {
        let (s, v) = data[i]
        continue (i + 1, trie.add(s, v))
      }
  }
}

///|
pub fn[A] Trie::from_array(data : Array[(String, A)]) -> Trie[A] {
  loop (0, Trie::empty()) {
    (i, trie) =>
      if i == data.length() {
        trie
      } else {
        let (s, v) = data[i]
        continue (i + 1, trie.add(s, v))
      }
  }
}
