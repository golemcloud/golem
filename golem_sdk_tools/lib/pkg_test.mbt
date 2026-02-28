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
  inspect(
    exports,
    content=(
      #|["cabi_realloc:cabi_realloc", "wasmExportInit:golem:agent/guest#initialize", "wasmExportSave:golem:api/save-snapshot@1.1.7#save"]
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
  inspect(
    result,
    content=(
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
      #|        "cabi_realloc:cabi_realloc",
      #|        "wasmExportInit:golem:agent/guest#initialize",
      #|      ],
      #|      "heap-start-address": 16,
      #|    },
      #|  },
      #|)
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
  inspect(
    result,
    content=(
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
      #|        "cabi_realloc:cabi_realloc",
      #|      ],
      #|      "heap-start-address": 16,
      #|    },
      #|  },
      #|)
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
    content, "vigoo/golem_sdk/agents/types", "@types",
  )
  inspect(
    result,
    content=(
      #|import {
      #|  "some/pkg",
      #|  "vigoo/golem_sdk/agents/types" @types,
      #|}
      #|
      #|options(
      #|  "is-main": true,
      #|)
    ),
  )
}

///|
test "ensure_pkg_import is idempotent" {
  let content =
    #|import {
    #|  "some/pkg",
    #|  "vigoo/golem_sdk/agents/types" @types,
    #|}
    #|
    #|options(
    #|  "is-main": true,
    #|)
  let result = @lib.ensure_pkg_import(
    content, "vigoo/golem_sdk/agents/types", "@types",
  )
  inspect(
    result,
    content=(
      #|import {
      #|  "some/pkg",
      #|  "vigoo/golem_sdk/agents/types" @types,
      #|}
      #|
      #|options(
      #|  "is-main": true,
      #|)
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
    content, "vigoo/golem_sdk/agents/types", "@types",
  )
  inspect(
    result,
    content=(
      #|import {
      #|  "vigoo/golem_sdk/agents/types" @types,
      #|}
      #|
      #|options(
      #|  "is-main": true,
      #|)
    ),
  )
}

///|
test "update_pkg_link appends options when none exist" {
  let content =
    #|import {
    #|  "some/pkg",
    #|}
  let exports = ["cabi_realloc:cabi_realloc"]
  let result = @lib.update_pkg_link(content, exports)
  inspect(
    result,
    content=(
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
      #|      ],
      #|      "heap-start-address": 16,
      #|    },
      #|  },
      #|)
    ),
  )
}
