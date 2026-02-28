# Trie

A trie, also known as a prefix tree, is a tree-like data structure that stores a collection of strings.
This implementation provides the basic functionality of a trie, including inserting strings and searching for strings.

# Usage

```moonbit
typealias @trie.Trie
test {
  let trie = Trie::of([("--search", "search"), ("--switch", "switch")])
  inspect(trie.lookup("--search"), content="Some(\"search\")")
  let trie = trie.add("-s", "s")
  inspect(trie.lookup("-s"), content="Some(\"s\")")
  inspect(trie.lookup("--switch"), content="Some(\"switch\")")
}
```
