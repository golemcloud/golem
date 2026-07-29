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
test "ensure_pkg_import upgrades an existing import to the generated alias" {
  let content =
    #|import {
    #|  "golemcloud/golem_sdk/tool" @sdk_tool,
    #|}
  let result = @lib.ensure_pkg_import(
    content, "golemcloud/golem_sdk/tool", "@tool",
  )
  assert_true(result.contains("\"golemcloud/golem_sdk/tool\" @tool,"))
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
test "ensure_pkg_import does not mistake an alias prefix for the exact alias" {
  let content =
    #|import {
    #|  "golemcloud/golem_sdk/tool" @toolCommon,
    #|}
  let result = @lib.ensure_pkg_import(
    content, "golemcloud/golem_sdk/tool", "@tool",
  )
  assert_true(result.contains("\"golemcloud/golem_sdk/tool\" @tool,"))
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
test "ensure_pkg_import preserves tab indentation when upgrading an alias" {
  let content = "import {\n\t\"golemcloud/golem_sdk/tool\" @sdk_tool // keep this explanation\n}"
  let result = @lib.ensure_pkg_import(
    content, "golemcloud/golem_sdk/tool", "@tool",
  )
  assert_eq(
    result, "import {\n\t\"golemcloud/golem_sdk/tool\" @tool, // keep this explanation\n}",
  )
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
