///|
test "parse_pkg_exports extracts export entries from moon.pkg" {
  let content =
    #|import {
    #|  "some/pkg",
    #|}
    #|
    #|options(
    #|  link: {
    #|    "wasm": {
    #|      "export-memory-name": "memory",
    #|      "exports": [
    #|        "cabi_realloc:cabi_realloc",
    #|        "wasmExportInit:golem:agent/guest#initialize",
    #|        "wasmExportSave:golem:api/save-snapshot@1.1.7#save",
    #|      ],
    #|      "heap-start-address": 16,
    #|    },
    #|  },
    #|)
  let exports = @lib.parse_pkg_exports(content)
  debug_inspect(
    exports,
    content=(
      #|[
      #|  "cabi_realloc:cabi_realloc",
      #|  "wasmExportInit:golem:agent/guest#initialize",
      #|  "wasmExportSave:golem:api/save-snapshot@1.1.7#save",
      #|]
    ),
  )
}

///|
test "update_pkg_link replaces existing link block" {
  let content =
    #|import {
    #|  "some/pkg",
    #|}
    #|
    #|options(
    #|  "is-main": true,
    #|  link: {
    #|    "wasm": {
    #|      "export-memory-name": "memory",
    #|      "exports": [
    #|        "old:old-export",
    #|      ],
    #|      "heap-start-address": 16,
    #|    },
    #|  },
    #|)
  let exports = [
    "cabi_realloc:cabi_realloc", "wasmExportInit:golem:agent/guest#initialize",
  ]
  let result = @lib.update_pkg_link(content, exports)
  debug_inspect(
    result,
    content=(
      #|"import {\n  \"some/pkg\",\n}\n\noptions(\n  \"is-main\": true,\n  link: {\n    \"wasm\": {\n      \"export-memory-name\": \"memory\",\n      \"exports\": [\n        \"cabi_realloc:cabi_realloc\",\n        \"wasmExportInit:golem:agent/guest#initialize\",\n      ],\n      \"heap-start-address\": 16,\n    },\n  },\n)"
    ),
  )
}

///|
test "update_pkg_link inserts link into existing options" {
  let content =
    #|import {
    #|  "some/pkg",
    #|}
    #|
    #|options(
    #|  "is-main": true,
    #|)
  let exports = ["cabi_realloc:cabi_realloc"]
  let result = @lib.update_pkg_link(content, exports)
  debug_inspect(
    result,
    content=(
      #|"import {\n  \"some/pkg\",\n}\n\noptions(\n  \"is-main\": true,\n  link: {\n    \"wasm\": {\n      \"export-memory-name\": \"memory\",\n      \"exports\": [\n        \"cabi_realloc:cabi_realloc\",\n      ],\n      \"heap-start-address\": 16,\n    },\n  },\n)"
    ),
  )
}

///|
test "ensure_pkg_import adds import to existing block" {
  let content =
    #|import {
    #|  "some/pkg",
    #|}
    #|
    #|options(
    #|  "is-main": true,
    #|)
  let result = @lib.ensure_pkg_import(
    content, "golemcloud/golem_sdk/schema", "@schema",
  )
  debug_inspect(
    result,
    content=(
      #|"import {\n  \"some/pkg\",\n  \"golemcloud/golem_sdk/schema\" @schema,\n}\n\noptions(\n  \"is-main\": true,\n)"
    ),
  )
}

///|
test "ensure_pkg_import is idempotent" {
  let content =
    #|import {
    #|  "some/pkg",
    #|  "golemcloud/golem_sdk/schema" @schema,
    #|}
    #|
    #|options(
    #|  "is-main": true,
    #|)
  let result = @lib.ensure_pkg_import(
    content, "golemcloud/golem_sdk/schema", "@schema",
  )
  debug_inspect(
    result,
    content=(
      #|"import {\n  \"some/pkg\",\n  \"golemcloud/golem_sdk/schema\" @schema,\n}\n\noptions(\n  \"is-main\": true,\n)"
    ),
  )
}

///|
test "ensure_pkg_import accepts a matching default alias" {
  let content =
    #|import {
    #|  "golemcloud/golem_sdk/tool",
    #|}
  let result = @lib.ensure_pkg_import(
    content, "golemcloud/golem_sdk/tool", "@tool",
  )
  assert_eq(result, content)
}

///|
test "ensure_pkg_import accepts a matching deprecated alias" {
  let content =
    #|import {
    #|  "golemcloud/golem_sdk/schema_model" as @model,
    #|}
  let result = @lib.ensure_pkg_import(
    content, "golemcloud/golem_sdk/schema_model", "@model",
  )
  assert_eq(result, content)
}

///|
test "ensure_pkg_import rejects an existing package under a different alias" {
  let content =
    #|import {
    #|  "golemcloud/golem_sdk/tool" @sdk_tool,
    #|}
  let error = try
    @lib.ensure_pkg_import(content, "golemcloud/golem_sdk/tool", "@tool")
  catch {
    @lib.ComponentRoleError(message) => message
  } noraise {
    _ => "NO ERROR"
  }
  inspect(
    error,
    content="moon.pkg package 'golemcloud/golem_sdk/tool' is already imported as '@sdk_tool'; generated source requires alias '@tool'",
  )
}

///|
test "ensure_pkg_import matches an exact package path" {
  let content =
    #|import {
    #|  "golemcloud/golem_sdk/tool/common" @toolCommon,
    #|}
  let result = @lib.ensure_pkg_import(
    content, "golemcloud/golem_sdk/tool", "@tool",
  )
  assert_true(
    result.contains(
      "  \"golemcloud/golem_sdk/tool/common\" @toolCommon,\n  \"golemcloud/golem_sdk/tool\" @tool,",
    ),
  )
}

///|
test "ensure_pkg_import rejects an alias prefix rather than rewriting it" {
  let content =
    #|import {
    #|  "golemcloud/golem_sdk/tool" @toolCommon,
    #|}
  let error = try
    @lib.ensure_pkg_import(content, "golemcloud/golem_sdk/tool", "@tool")
  catch {
    @lib.ComponentRoleError(message) => message
  } noraise {
    _ => "NO ERROR"
  }
  assert_true(error.contains("is already imported as '@toolCommon'"))
}

///|
test "ensure_pkg_import preserves an exact aliased import with a trailing comment" {
  let content =
    #|import {
    #|  "golemcloud/golem_sdk/tool" @tool, // keep this explanation
    #|}
  let result = @lib.ensure_pkg_import(
    content, "golemcloud/golem_sdk/tool", "@tool",
  )
  assert_eq(result, content)
}

///|
test "ensure_pkg_import does not rewrite a tab-indented user alias" {
  let content = "import {\n\t\"golemcloud/golem_sdk/tool\" @sdk_tool // keep this explanation\n}"
  let error = try
    @lib.ensure_pkg_import(content, "golemcloud/golem_sdk/tool", "@tool")
  catch {
    @lib.ComponentRoleError(message) => message
  } noraise {
    _ => "NO ERROR"
  }
  assert_true(error.contains("is already imported as '@sdk_tool'"))
}

///|
test "ensure_pkg_import repairs a comma-less final entry before insertion" {
  let content =
    #|import {
    #|  "some/pkg" // keep this explanation
    #|}
  let result = @lib.ensure_pkg_import(
    content, "golemcloud/golem_sdk/tool", "@tool",
  )
  assert_eq(
    result,
    (
      #|import {
      #|  "some/pkg", // keep this explanation
      #|  "golemcloud/golem_sdk/tool" @tool,
      #|}
    ),
  )
}

///|
test "ensure_pkg_import ignores a commented-out import" {
  let content =
    #|import {
    #|  "some/pkg",
    #|  // "golemcloud/golem_sdk/tool" @old_tool,
    #|}
  let result = @lib.ensure_pkg_import(
    content, "golemcloud/golem_sdk/tool", "@tool",
  )
  assert_true(
    result.contains(
      "  // \"golemcloud/golem_sdk/tool\" @old_tool,\n  \"golemcloud/golem_sdk/tool\" @tool,",
    ),
  )
}

///|
test "ensure_pkg_import does not rewrite matching strings outside the import block" {
  let content =
    #|import {
    #|  "some/pkg",
    #|}
    #|
    #|options(
    #|  "pre-build": [
    #|    "golemcloud/golem_sdk/tool",
    #|  ],
    #|)
  let result = @lib.ensure_pkg_import(
    content, "golemcloud/golem_sdk/tool", "@tool",
  )
  assert_true(
    result.contains(
      "  \"some/pkg\",\n  \"golemcloud/golem_sdk/tool\" @tool,\n}",
    ),
  )
  assert_true(
    result.contains(
      "  \"pre-build\": [\n    \"golemcloud/golem_sdk/tool\",\n  ]",
    ),
  )
}

///|
test "ensure_pkg_import leaves postfix test imports unchanged" {
  for qualifier in ["test", "wbtest"] {
    let content = "import {\n  \"golemcloud/golem_sdk/tool\" @test_tool,\n} for \"\{qualifier}\""
    let result = @lib.ensure_pkg_import(
      content, "golemcloud/golem_sdk/tool", "@tool",
    )
    assert_eq(
      result,
      "import {\n  \"golemcloud/golem_sdk/tool\" @tool,\n}\n\n" + content,
    )
  }
}

///|
test "ensure_pkg_import leaves legacy test imports unchanged" {
  for qualifier in ["test", "wbtest"] {
    let content = "import \"\{qualifier}\" {\n  \"golemcloud/golem_sdk/tool\" @test_tool,\n}"
    let result = @lib.ensure_pkg_import(
      content, "golemcloud/golem_sdk/tool", "@tool",
    )
    assert_eq(
      result,
      "import {\n  \"golemcloud/golem_sdk/tool\" @tool,\n}\n\n" + content,
    )
  }
}

///|
test "ensure_pkg_import finds a regular block after a test block" {
  let content =
    #|import {
    #|  "test/pkg",
    #|} for "test"
    #|
    #|import {
    #|  "regular/pkg",
    #|}
  let result = @lib.ensure_pkg_import(
    content, "golemcloud/golem_sdk/tool", "@tool",
  )
  assert_eq(
    result,
    (
      #|import {
      #|  "test/pkg",
      #|} for "test"
      #|
      #|import {
      #|  "regular/pkg",
      #|  "golemcloud/golem_sdk/tool" @tool,
      #|}
    ),
  )
}

///|
test "ensure_pkg_import inserts into a comment-only regular block" {
  let content =
    #|import { // regular imports
    #|  // Imports are generated below.
    #|} // end regular imports
  let result = @lib.ensure_pkg_import(
    content, "golemcloud/golem_sdk/tool", "@tool",
  )
  assert_eq(
    result,
    (
      #|import { // regular imports
      #|  // Imports are generated below.
      #|  "golemcloud/golem_sdk/tool" @tool,
      #|} // end regular imports
    ),
  )
}

///|
test "ensure_pkg_import adds import block when none exists" {
  let content =
    #|options(
    #|  "is-main": true,
    #|)
  let result = @lib.ensure_pkg_import(
    content, "golemcloud/golem_sdk/schema", "@schema",
  )
  debug_inspect(
    result,
    content=(
      #|"import {\n  \"golemcloud/golem_sdk/schema\" @schema,\n}\n\noptions(\n  \"is-main\": true,\n)"
    ),
  )
}

///|
test "ensure_pkg_import is idempotent across the generated tool imports" {
  let imports = [
    ("golemcloud/golem_sdk/tool", "@tool"),
    ("golemcloud/golem_sdk/schema", "@schema"),
    ("golemcloud/golem_sdk/schema_model", "@model"),
    ("golemcloud/golem_sdk/interface/golem/core/types", "@types"),
    ("golemcloud/golem_sdk/interface/golem/tool/common", "@toolCommon"),
    ("golemcloud/golem_sdk/interface/wasi/io/streams", "@streams"),
  ]
  let mut once = "options(\n  \"is-main\": true,\n)"
  for item in imports {
    let (path, import_alias) = item
    once = @lib.ensure_pkg_import(once, path, import_alias)
  }
  let mut twice = once
  for item in imports {
    let (path, import_alias) = item
    twice = @lib.ensure_pkg_import(twice, path, import_alias)
  }
  assert_eq(twice, once)
}

///|
test "ensure_role_gen_import replaces a previous role without duplicating @gen" {
  let content =
    #|import {
    #|  "golemcloud/golem_sdk/gen" @gen,
    #|  "some/pkg" @other,
    #|}
  let result = @lib.ensure_role_gen_import(
    content, "golemcloud/golem_sdk/gen-tool-middleware",
  )
  inspect(
    result,
    content=(
      #|import {
      #|  "golemcloud/golem_sdk/gen-tool-middleware" @gen,
      #|  "some/pkg" @other,
      #|}
    ),
  )
  assert_eq(
    @lib.ensure_role_gen_import(
      result, "golemcloud/golem_sdk/gen-tool-middleware",
    ),
    result,
  )
}

///|
test "ensure_role_gen_import normalizes an existing selected package alias" {
  let content =
    #|import {
    #|  "golemcloud/golem_sdk/gen" @gen,
    #|  "golemcloud/golem_sdk/gen-agent-tool-middleware" @combined,
    #|}
  inspect(
    @lib.ensure_role_gen_import(
      content, "golemcloud/golem_sdk/gen-agent-tool-middleware",
    ),
    content=(
      #|import {
      #|  "golemcloud/golem_sdk/gen-agent-tool-middleware" @gen,
      #|}
    ),
  )
}

///|
test "ensure_role_gen_import rejects unrelated @gen alias ownership" {
  let content =
    #|import {
    #|  "some/generator" @gen,
    #|}
  let error = try
    @lib.ensure_role_gen_import(content, "golemcloud/golem_sdk/gen")
  catch {
    @lib.ComponentRoleError(message) => message
  } noraise {
    _ => "NO ERROR"
  }
  inspect(
    error,
    content="moon.pkg import alias @gen is already used by unrelated package 'some/generator'",
  )
}

///|
test "ensure_role_tool_import preserves exact imports and rejects role conflicts" {
  let content =
    #|import {
    #|  "golemcloud/golem_sdk/tool" @tool,
    #|  "some/pkg" @other,
    #|}
  assert_eq(
    @lib.ensure_role_tool_import(content, "golemcloud/golem_sdk/tool"),
    content,
  )
  let error = try
    @lib.ensure_role_tool_import(content, "golemcloud/golem_sdk/tool-core")
  catch {
    @lib.ComponentRoleError(message) => message
  } noraise {
    _ => "NO ERROR"
  }
  inspect(
    error,
    content="moon.pkg role tool package 'golemcloud/golem_sdk/tool' conflicts with required package 'golemcloud/golem_sdk/tool-core'",
  )
}

///|
test "ensure_role_tool_import rejects selected-package and unrelated alias conflicts" {
  let content =
    #|import {
    #|  "golemcloud/golem_sdk/tool" @ambient,
    #|}
  let selected_error = try
    @lib.ensure_role_tool_import(content, "golemcloud/golem_sdk/tool")
  catch {
    @lib.ComponentRoleError(message) => message
  } noraise {
    _ => "NO ERROR"
  }
  assert_true(selected_error.contains("is already imported as '@ambient'"))
  let error = try
    @lib.ensure_role_tool_import(
      "import {\n  \"some/tool\" @tool,\n}", "golemcloud/golem_sdk/tool-core",
    )
  catch {
    @lib.ComponentRoleError(message) => message
  } noraise {
    _ => "NO ERROR"
  }
  inspect(
    error,
    content="moon.pkg import alias @tool is already used by unrelated package 'some/tool'",
  )
}

///|
test "generated package imports are deterministic and stale owned entries are removed" {
  let original =
    #|import {
    #|  "user/owned" @userOwned,
    #|  "golemcloud/golem_sdk/schema" @schema,
    #|}
    #|
    #|options(
    #|  "is-main": true,
    #|)
  let monomorphic = [
    ("golemcloud/golem_sdk/tool-middleware", "@toolMiddleware"),
    ("golemcloud/golem_sdk/tool-core", "@tool"),
    ("golemcloud/golem_sdk/schema", "@schema"),
  ]
  let once = @lib.reconcile_generated_pkg_imports(original, monomorphic)
  assert_true(
    once.contains(
      "\"golemcloud/golem_sdk/tool-middleware\" @toolMiddleware, // Generated by golem_sdk_tools",
    ),
  )
  assert_true(
    once.contains(
      "\"golemcloud/golem_sdk/tool-core\" @tool, // Generated by golem_sdk_tools",
    ),
  )
  assert_false(
    once.contains(
      "\"golemcloud/golem_sdk/schema\" @schema, // Generated by golem_sdk_tools",
    ),
  )
  assert_eq(@lib.reconcile_generated_pkg_imports(once, monomorphic), once)
  let universal = [("golemcloud/golem_sdk/tool-middleware", "@toolMiddleware")]
  let cleaned = @lib.reconcile_generated_pkg_imports(once, universal)
  assert_true(cleaned.contains("\"user/owned\" @userOwned,"))
  assert_true(cleaned.contains("\"golemcloud/golem_sdk/schema\" @schema,"))
  assert_false(cleaned.contains("golemcloud/golem_sdk/tool-core"))
  assert_eq(cleaned.split("// Generated by golem_sdk_tools").count() - 1, 1)
}

///|
test "generated package imports switch owned tool roles without duplicate aliases" {
  let pure = @lib.reconcile_generated_pkg_imports(
    "options(\n  \"is-main\": true,\n)",
    [("golemcloud/golem_sdk/tool-core", "@tool")],
  )
  let combined = @lib.reconcile_generated_pkg_imports(pure, [
    ("golemcloud/golem_sdk/tool", "@tool"),
  ])
  assert_false(combined.contains("golemcloud/golem_sdk/tool-core"))
  assert_eq(combined.split("@tool,").count() - 1, 1)
  assert_true(
    combined.contains(
      "\"golemcloud/golem_sdk/tool\" @tool, // Generated by golem_sdk_tools",
    ),
  )
}

///|
test "generated import reconciliation preserves user ownership and rejects conflicts" {
  let custom_alias =
    #|import {
    #|  "golemcloud/golem_sdk/schema" @customSchema,
    #|}
  let path_error = try
    @lib.reconcile_generated_pkg_imports(custom_alias, [
      ("golemcloud/golem_sdk/schema", "@schema"),
    ])
  catch {
    @lib.ComponentRoleError(message) => message
  } noraise {
    _ => "NO ERROR"
  }
  inspect(
    path_error,
    content="moon.pkg package 'golemcloud/golem_sdk/schema' is already imported as '@customSchema'; generated source requires alias '@schema'",
  )
  let alias_error = try
    @lib.reconcile_generated_pkg_imports(
      "import {\n  \"user/schema\" @schema,\n}",
      [("golemcloud/golem_sdk/schema", "@schema")],
    )
  catch {
    @lib.ComponentRoleError(message) => message
  } noraise {
    _ => "NO ERROR"
  }
  inspect(
    alias_error,
    content="moon.pkg import alias @schema is already used by unrelated package 'user/schema'",
  )
}

///|
test "pure generated imports reject ambient tool under every user alias" {
  let error = try
    @lib.reconcile_generated_pkg_imports(
      "import {\n  \"golemcloud/golem_sdk/tool\" @customTool,\n}",
      [("golemcloud/golem_sdk/tool-core", "@tool")],
      forbid_ambient_tool=true,
    )
  catch {
    @lib.ComponentRoleError(message) => message
  } noraise {
    _ => "NO ERROR"
  }
  inspect(
    error,
    content="Pure tool-middleware moon.pkg must not import ambient package 'golemcloud/golem_sdk/tool'",
  )
}

///|
test "pure role reconciliation removes only owned ambient tool imports" {
  let content =
    #|import {
    #|  "user/owned" @userOwned,
    #|  "golemcloud/golem_sdk/tool" @tool, // Generated by golem_sdk_tools
    #|}
  let reconciled = @lib.reconcile_pure_role_pkg_imports(content)
  assert_true(reconciled.contains("\"user/owned\" @userOwned,"))
  assert_false(reconciled.contains("golemcloud/golem_sdk/tool"))
}

///|
test "generated import markers outside regular entries do not confer ownership" {
  let content =
    #|import {
    #|  "user/owned" @userOwned, // Generated by golem_sdk_tools for documentation
    #|}
    #|
    #|// Generated by golem_sdk_tools
    #|options(
    #|  "is-main": true,
    #|)
  assert_eq(@lib.reconcile_generated_pkg_imports(content, []), content)
}

///|
test "update_pkg_link appends options when none exist" {
  let content =
    #|import {
    #|  "some/pkg",
    #|}
  let exports = ["cabi_realloc:cabi_realloc"]
  let result = @lib.update_pkg_link(content, exports)
  debug_inspect(
    result,
    content=(
      #|"import {\n  \"some/pkg\",\n}\n\noptions(\n  link: {\n    \"wasm\": {\n      \"export-memory-name\": \"memory\",\n      \"exports\": [\n        \"cabi_realloc:cabi_realloc\",\n      ],\n      \"heap-start-address\": 16,\n    },\n  },\n)"
    ),
  )
}
