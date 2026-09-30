///|
test "export hooks follow all eight source-category combinations" {
  for agents in [false, true] {
    for tools in [false, true] {
      for middlewares in [false, true] {
        let (impls, imports) = @lib.generate_export_hooks(
          agents, tools, middlewares,
        )
        let source = @formatter.impls_to_string(impls)
        assert_eq(source.contains("@agentExports.initialize"), agents)
        assert_eq(source.contains("@agentExports.save"), agents)
        assert_eq(source.contains("@agentExports.load"), agents)
        assert_eq(source.contains("@toolExports.invoke"), tools)
        assert_eq(
          source.contains("@middlewareExports.invoke_tool_middleware"),
          middlewares,
        )
        assert_eq(
          imports.length(),
          (if agents { 4 } else { 0 }) +
          (if tools { 2 } else { 0 }) +
          (if middlewares { 2 } else { 0 }),
        )
        let original = "import {\n}\noptions(\n  \"is-main\": true,\n)\n"
        let wired = @lib.reconcile_generated_pkg_imports(original, imports)
        assert_eq(@lib.reconcile_generated_pkg_imports(wired, []), original)
      }
    }
  }
}
