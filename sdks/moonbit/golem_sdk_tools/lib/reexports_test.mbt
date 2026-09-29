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
  debug_inspect(
    output,
    content=(
      #|"///|\npub fn wasmExportSave() -> Int {\n  @gen.wasmExportSave()\n}\n\n"
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
  debug_inspect(
    output,
    content=(
      #|"///|\npub fn cabi_realloc(p0 : Int, p1 : Int, p2 : Int, p3 : Int) -> Int {\n  @gen.mbt_ffi_cabi_realloc(p0, p1, p2, p3)\n}\n\n"
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
  debug_inspect(
    output,
    content=(
      #|"///|\npub fn wasmExportSave() -> Int {\n  @gen.wasmExportSave()\n}\n\n///|\npub fn wasmExportSavePostReturn(p0 : Int) -> Unit {\n  @gen.wasmExportSavePostReturn(p0)\n}\n\n"
    ),
  )
}

///|
test "component exports select raw ABI independently for each capability" {
  let signatures = parse_mbt_source(
    (
      #|pub fn wasmExportDiscoverAgentTypes() -> Int { 0 }
      #|pub fn wasmExportGetTool(p0 : Int, p1 : Int) -> Int { 0 }
      #|pub fn wasmExportInvokeToolMiddleware(p0 : Int) -> Int { 0 }
      #|pub fn wasmExportAsyncInvokeToolMiddleware(p0 : Int, p1 : Int, p2 : Int) -> Int { 0 }
      #|pub fn wasmExportLoad(p0 : Int, p1 : Int, p2 : Int, p3 : Int) -> Int { 0 }
      #|pub fn mbt_ffi_cabi_realloc(p0 : Int, p1 : Int, p2 : Int, p3 : Int) -> Int { 0 }
    ),
  )
  for bits in 0..<8 {
    let agent = (bits & 1) != 0
    let tool = (bits & 2) != 0
    let middleware = (bits & 4) != 0
    let output = @formatter.impls_to_string(
      generate_component_reexports(signatures, agent, tool, middleware),
    )
    assert_eq(output.contains("@gen.wasmExportDiscoverAgentTypes()"), agent)
    assert_eq(output.contains("@minimalExports.discover()"), !agent)
    assert_eq(output.contains("@gen.wasmExportGetTool(p0, p1)"), tool)
    assert_eq(output.contains("@minimalExports.get(p0, p1)"), !tool)
    assert_eq(
      output.contains("@gen.wasmExportInvokeToolMiddleware(p0)"),
      middleware,
    )
    assert_eq(output.contains("@minimalExports.invoke(p0)"), !middleware)
    assert_eq(
      output.contains("@minimalExports.callback(p0, p1, p2)"),
      !middleware,
    )
    assert_eq(output.contains("@minimalExports.load(p0, p1, p2, p3)"), !agent)
    assert_true(output.contains("@gen.mbt_ffi_cabi_realloc(p0, p1, p2, p3)"))
    // Replacing bodies must not change any exported core signature.
    assert_eq(parse_mbt_source(output).length(), signatures.length())
    for i in 0..<signatures.length() {
      let actual = parse_mbt_source(output)[i]
      assert_eq(actual.user_name, signatures[i].user_name)
      assert_eq(actual.params, signatures[i].params)
      assert_eq(actual.return_type, signatures[i].return_type)
    }
  }
}
