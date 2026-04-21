///|
test "generate_reexports produces correct output for simple function" {
  let fns : Array[FnSignature] = [
    {
      gen_name: "wasmExportSave",
      user_name: "wasmExportSave",
      params: [],
      return_type: "Int",
    },
  ]
  let impls = generate_reexports(fns, "gen")
  let output = @formatter.impls_to_string(impls)
  inspect(
    output,
    content=(
      #|///|
      #|pub fn wasmExportSave() -> Int {
      #|  @gen.wasmExportSave()
      #|}
      #|
      #|
    ),
  )
}

///|
test "generate_reexports produces correct output for function with params" {
  let fns : Array[FnSignature] = [
    {
      gen_name: "mbt_ffi_cabi_realloc",
      user_name: "cabi_realloc",
      params: ["Int", "Int", "Int", "Int"],
      return_type: "Int",
    },
  ]
  let impls = generate_reexports(fns, "gen")
  let output = @formatter.impls_to_string(impls)
  inspect(
    output,
    content=(
      #|///|
      #|pub fn cabi_realloc(p0 : Int, p1 : Int, p2 : Int, p3 : Int) -> Int {
      #|  @gen.mbt_ffi_cabi_realloc(p0, p1, p2, p3)
      #|}
      #|
      #|
    ),
  )
}

///|
test "generate_reexports multiple functions" {
  let fns : Array[FnSignature] = [
    {
      gen_name: "wasmExportSave",
      user_name: "wasmExportSave",
      params: [],
      return_type: "Int",
    },
    {
      gen_name: "wasmExportSavePostReturn",
      user_name: "wasmExportSavePostReturn",
      params: ["Int"],
      return_type: "Unit",
    },
  ]
  let impls = generate_reexports(fns, "gen")
  let output = @formatter.impls_to_string(impls)
  inspect(
    output,
    content=(
      #|///|
      #|pub fn wasmExportSave() -> Int {
      #|  @gen.wasmExportSave()
      #|}
      #|
      #|///|
      #|pub fn wasmExportSavePostReturn(p0 : Int) -> Unit {
      #|  @gen.wasmExportSavePostReturn(p0)
      #|}
      #|
      #|
    ),
  )
}
