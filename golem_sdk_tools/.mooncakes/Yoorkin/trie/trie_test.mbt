///|
test {
  let trie = @trie.T::of([
    ("--search", "search"),
    ("-s", "s"),
    ("--switch", "switch"),
  ])
  assert_eq(trie.lookup("--search"), Some("search"))
  assert_eq(trie.lookup("-s"), Some("s"))
  assert_eq(trie.lookup("--switch"), Some("switch"))
}
