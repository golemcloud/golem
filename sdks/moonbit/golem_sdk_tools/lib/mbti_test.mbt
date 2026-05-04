///|
test "parse_mbt_source parses pub fn with return type" {
  let content =
    #|///|
    #|pub fn wasmExportSave() -> Int {
    #|  42
    #|}
  let fns = parse_mbt_source(content)
  inspect(fns.length(), content="1")
  inspect(fns[0].gen_name, content="wasmExportSave")
  inspect(fns[0].user_name, content="wasmExportSave")
  inspect(fns[0].params.length(), content="0")
  inspect(fns[0].return_type, content="Int")
}

///|
test "parse_mbt_source parses pub fn with params" {
  let content =
    #|///|
    #|pub fn wasmExportInitialize(p0 : Int, p1 : Int, p2 : Int, p3 : Int, p4 : Int) -> Int {
    #|  0
    #|}
  let fns = parse_mbt_source(content)
  inspect(fns.length(), content="1")
  inspect(fns[0].gen_name, content="wasmExportInitialize")
  inspect(fns[0].params.length(), content="5")
  inspect(fns[0].params[0], content="Int")
}

///|
test "parse_mbt_source parses pub fn returning Unit" {
  let content =
    #|///|
    #|pub fn wasmExportSavePostReturn(p0 : Int) -> Unit {
    #|  ()
    #|}
  let fns = parse_mbt_source(content)
  inspect(fns.length(), content="1")
  inspect(fns[0].return_type, content="Unit")
}

///|
test "parse_mbt_source strips mbt_ffi_ prefix for user_name" {
  let content =
    #|///|
    #|pub fn mbt_ffi_cabi_realloc(src_offset : Int, src_size : Int, _dst_alignment : Int, dst_size : Int) -> Int {
    #|  0
    #|}
  let fns = parse_mbt_source(content)
  inspect(fns.length(), content="1")
  inspect(fns[0].gen_name, content="mbt_ffi_cabi_realloc")
  inspect(fns[0].user_name, content="cabi_realloc")
}

///|
test "parse_mbt_source skips non-pub and extern fns" {
  let content =
    #|///|
    #|pub fn wasmExportSave() -> Int {
    #|  42
    #|}
    #|
    #|///|
    #|fn private_helper() -> Int {
    #|  0
    #|}
    #|
    #|///|
    #|let return_area : Int = 0
  let fns = parse_mbt_source(content)
  inspect(fns.length(), content="1")
  inspect(fns[0].user_name, content="wasmExportSave")
}

///|
test "parse_mbt_source handles multiple pub fns" {
  let content =
    #|///|
    #|pub fn mbt_ffi_cabi_realloc(p0 : Int, p1 : Int, p2 : Int, p3 : Int) -> Int {
    #|  0
    #|}
    #|
    #|///|
    #|pub fn wasmExportSave() -> Int {
    #|  42
    #|}
    #|
    #|///|
    #|pub fn wasmExportSavePostReturn(p0 : Int) -> Unit {
    #|  ()
    #|}
  let fns = parse_mbt_source(content)
  inspect(fns.length(), content="3")
  inspect(fns[0].user_name, content="cabi_realloc")
  inspect(fns[1].user_name, content="wasmExportSave")
  inspect(fns[2].user_name, content="wasmExportSavePostReturn")
}
