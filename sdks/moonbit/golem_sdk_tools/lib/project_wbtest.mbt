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

///|
test "generated middleware output is excluded from source scans" {
  assert_true(generated_files.contains("golem_tool_middlewares.mbt"))
}

///|
test "WIT dependency trees are excluded from package discovery" {
  assert_true(skip_dirs.contains("wit"))
}
