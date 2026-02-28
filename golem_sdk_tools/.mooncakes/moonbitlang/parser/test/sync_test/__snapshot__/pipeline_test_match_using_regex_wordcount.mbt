///|
fn count(
  input : BytesView,
  lines : Int,
  words : Int,
  chars : Int
) -> (Int, Int, Int) {
  lexmatch input with longest {
    ("\n", next) => count(next, lines + 1, words, chars)
    ("[^ \t\r\n]+" as word, next) =>
      count(next, lines, words + 1, chars + word.length())
    (".", next) => count(next, lines, words, chars + 1)
    "" => (lines, words, chars)
    _ => panic()
  }
}

///|
fn main {
  println(
    count(
      b"Non enim laborum velit commodo deserunt incididunt elit sunt nulla ullamco. Quis ipsum aliqua id mollit minim velit et cupidatat eiusmod nostrud pariatur duis irure ad. Dolore enim et elit fugiat.\n" +
      b"Nostrud elit minim aute qui labore id aute aute ea nostrud cupidatat. Aliquip et commodo anim dolor nostrud voluptate proident. Voluptate tempor amet consequat nisi excepteur aute anim aute. Sunt ipsum tempor esse consequat cupidatat.\n" +
      b"Ipsum minim cillum adipisicing incididunt incididunt qui non excepteur mollit qui. Non aute sunt dolore eu sunt ea aute nisi dolor eiusmod. Fugiat id culpa exercitation consectetur cupidatat. Sunt sint nostrud dolor aute sit cupidatat voluptate reprehenderit ut cillum nulla. Dolore sunt elit elit et quis qui.\n",
      0,
      0,
      0,
    ),
  )
}
