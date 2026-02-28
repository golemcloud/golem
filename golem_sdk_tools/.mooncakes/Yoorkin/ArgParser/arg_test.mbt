///|
test {
  let verbose : Ref[Bool] = @ref.new(true)
  let keyword : Ref[String] = @ref.new("")
  let delete_files : Ref[Bool] = @ref.new(false)
  let usage =
    #| Awesome CLI tool!
    #| usage: 
    #|      mytool [options] <file1> [<file2>] ... -o <output>
    #|
    #|
  let files = []
  let options = [
    ("--no-verbose", "-n", @ArgParser.Clear(verbose), "disable verbose message"),
    ("--search", "-s", Set_string(keyword), "search for files"),
    ("--delete", "-d", Set(delete_files), "delete listed files"),
  ]
  let argv = ["--search", ".mbt", "--delete", "file1", "file2", "-n"]
  let _ = try? @ArgParser.parse(options, file => files.push(file), usage, argv)
  assert_eq(verbose.val, false)
  assert_eq(keyword.val, ".mbt")
  assert_eq(files.length(), 2)
  assert_eq(files[0], "file1")
  assert_eq(files[1], "file2")
}
