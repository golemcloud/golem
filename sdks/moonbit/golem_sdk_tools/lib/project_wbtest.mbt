///|
test "parent_dir of nested path" {
  debug_inspect(
    parent_dir("/a/b/c"),
    content=(
      #|"/a/b"
    ),
  )
}

///|
test "parent_dir of top-level path" {
  debug_inspect(
    parent_dir("/a"),
    content=(
      #|"/"
    ),
  )
}

///|
test "parent_dir of single segment without slash" {
  debug_inspect(
    parent_dir("a"),
    content=(
      #|"a"
    ),
  )
}
