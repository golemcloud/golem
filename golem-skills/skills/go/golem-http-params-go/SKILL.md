---
name: golem-http-params-go
description: "Mapping HTTP request elements to Go agent method inputs. Use when the user asks about path variables, query parameters, header mapping, request body mapping, supported parameter types, or response mapping for HTTP endpoints in a Go Golem project."
---

# HTTP Request and Response Parameter Mapping (Go)

## Overview

When a Go agent is exposed over HTTP (see `golem-add-http-endpoint-go`), the platform maps parts of each request onto the agent's constructor (`ID`) fields and the method's **input struct** fields. This skill covers how path segments, query parameters, headers, and the request body bind to those fields, which Go types each binding accepts, and how return types map to HTTP responses.

Binding is **metadata only** and declared on the definition: the mount `Path` on `Spec.HTTP` and the `golem.HTTP(...)` option on each method. There is no incoming-request handler in the guest.

## Steps

1. **Name fields to match wire names.** A path/query/header variable refers to an input field by its **lower-first** name: Go field `Sku` is `sku` on the wire, `MaxResults` is `maxResults`.
2. **Bind path variables** by putting `{name}` in the mount path (binds `ID` fields) or an endpoint path suffix (binds method input fields).
3. **Bind query and header values** inline (`?name={field}`) or with `golem.Query` / `golem.Header`.
4. **Leave body fields unbound** on body-carrying verbs; they arrive as JSON.

## Path variables

`{var}` segments bind by (lower-first) field name.

- **Mount path** `{var}`s bind the agent's constructor (`ID`) fields. **Every `ID` field must appear as a `{var}`** so a request selects one instance.
- **Endpoint path** `{var}`s bind the method's input fields.

```go
type ID struct{ Name string } // bound by {name} in the mount

type LookupIn struct {
	Sku      string
	Detailed bool
}

var Agent = golem.DefineAgent[ID](golem.Spec{
	Name: "CatalogAgent",
	HTTP: &golem.Mount{Path: "/catalog/{name}"}, // {name} → ID.Name
})

// GET /catalog/{name}/items/{sku}  — {sku} → LookupIn.Sku
var Lookup = golem.DefineMethod[ID, LookupIn, ItemInfo]("lookup",
	golem.HTTP(golem.GET("/items/{sku}", golem.Query("detailed", "detailed"))))
```

The trailing catch-all `{*rest}` captures everything after a prefix and may only appear as the **last** endpoint segment; it is **not** allowed in a mount path.

## Query parameters

Two equivalent forms. Inline `?name={field}` in the path suffix, or `golem.Query(wire, field)` when the wire name differs from the field:

```go
type SearchIn struct {
	Query      string
	MaxResults uint64
}

// GET /catalog/{name}/search?q=hello&limit=10
var Search = golem.DefineMethod[ID, SearchIn, []Result]("search",
	golem.HTTP(golem.GET("/search",
		golem.Query("q", "query"),         // ?q= → SearchIn.Query
		golem.Query("limit", "maxResults"), // ?limit= → SearchIn.MaxResults
	)))
```

Inline `golem.GET("/search?q={query}&limit={maxResults}")` is equivalent when the parameter name matches the field's wire name.

## Header variables

`golem.Header(wire, field)` maps a request header to an input field:

```go
type DataIn struct {
	RequestID string
	Token     string
}

var GetData = golem.DefineMethod[ID, DataIn, Data]("getData",
	golem.HTTP(golem.GET("/data",
		golem.Header("X-Request-Id", "requestId"),
		golem.Header("Authorization", "token"),
	)))
```

Header names are matched case-insensitively; binding the same header twice on one endpoint is a definition error.

## Supported types for path, query, and header bindings

A value bound from a URL path/query/header string must decode into a **scalar** Go field. Only these kinds bind:

| Go type | Bound from |
|---|---|
| `string` | used as-is |
| `bool` | `"true"` / `"false"` |
| `int8`, `int16`, `int32`, `int64` | signed integer |
| `uint8`, `uint16`, `uint32`, `uint64` | unsigned integer |
| `float32`, `float64` | floating-point number |

Binding a non-scalar field (struct, slice, map) from path/query/header is a definition error: *"cannot bind input field … only scalar fields decode from a URL"*. Such fields must arrive in the body instead.

> Prefer sized integers throughout. Bare `int`/`uint` are rejected at the agent-schema level because their width is platform-dependent (see `golem-add-agent-go`).

## Request body mapping

For body-carrying verbs (`POST`, `PUT`, `DELETE`, `PATCH`, …), any input field **not** bound to a path/query/header binding is populated from the JSON request body:

```go
type UpdateIn struct {
	Sku   string // from the path
	Name  string // from the body
	Count uint64 // from the body
}

// POST /catalog/{name}/items/{sku}   Body: {"name":"Widget","count":5}
var Update = golem.DefineMethod[ID, UpdateIn, Item]("update",
	golem.HTTP(golem.POST("/items/{sku}")))
```

Each unbound field becomes a top-level key in the JSON body object (field names lower-first / camelCase, as elsewhere in the SDK).

`GET` and `HEAD` are **bodyless**: they carry no request body, so they must bind **every** input field from path/query/header. An unbound field on a bodyless verb is a definition error.

## Return type to HTTP response mapping

Handlers return their output value; the platform maps it to a response. In Go:

| Return type | HTTP status | Body |
|---|---|---|
| `golem.Unit` | 204 No Content | empty |
| `T` (any type) | 200 OK | JSON-serialized `T` |
| `*T` | 200 OK if non-nil, 404 Not Found if nil | JSON `T` or empty |
| `golem.Result[T, E]` | 200 OK on `Ok`, 500 Internal Server Error on `Err` | JSON `T` or JSON `E` |

`golem.Unit` is the empty result placeholder; `*T` is Golem's `option<T>` (a nil pointer means "absent"); `golem.Result[Ok, Err]` returns the error as an observable value (see `golem-add-agent-go` on returning failures).

## Data type to JSON mapping

| Go type | JSON |
|---|---|
| `string` | string |
| sized ints / uints | number (integer) |
| `float32`, `float64` | number |
| `bool` | boolean |
| `[]T` | array |
| struct | object (lower-first / camelCase field names) |
| `*T` | value or `null` |
| `map[K]V` | object / WIT map |

## Key Constraints

- A variable can bind an input field from **exactly one** of path/query/header — binding it more than once is a definition error.
- Bindable path/query/header fields must be scalar; non-scalars go in the body.
- Bodyless verbs (`GET`, `HEAD`) must bind every input field.
- Field references on the wire use the **lower-first** field name (`Sku` → `sku`).
- **Go gap:** the Rust SDK's `UnstructuredBinary` / `UnstructuredText` (with `AllowedMimeTypes` / `AllowedLanguages`) raw-body wrapper types have **no public Go equivalent** yet — model bodies as JSON structs, or use `[]byte` fields for raw bytes.

### Related Skills

| Skill | When to Load |
|---|---|
| `golem-add-http-endpoint-go` | The high-level workflow of mounting an agent and adding routes |
| `golem-add-http-auth-go` | Require authentication on endpoints |
| `golem-add-cors-go` | Allow cross-origin requests |
| `golem-add-agent-go` | Define the agent and its input/output types |
