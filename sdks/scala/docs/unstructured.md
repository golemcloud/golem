# Unstructured text and binary values

`golem.schema.UnstructuredText[L]` and `golem.schema.UnstructuredBinary[M]` carry free-form content
that is not described by a structured schema: documents, images, generated files. They match the
Rust SDK's `UnstructuredText<LC>` / `UnstructuredBinary<MT>` and the TypeScript SDK's
`unstructuredText()` / `unstructuredBinary()` markers.

Each value is either inline content or a URL that refers to the content:

```scala
import golem.schema.{UnstructuredBinary, UnstructuredText}

UnstructuredText.fromInline("Hello")                  // Inline("Hello", None)
UnstructuredText.fromInline("Hallo", "de")            // Inline("Hallo", Some("de"))
UnstructuredText.fromUrl("https://example.com/a.txt") // Url(...)

UnstructuredBinary.fromInline(bytes)                  // Inline(bytes, None)
UnstructuredBinary.fromInline(bytes, "image/png")     // Inline(bytes, Some("image/png"))
UnstructuredBinary.fromUrl("https://example.com/a.png")
```

Pattern match on `UnstructuredText.Inline(text, languageCode)` / `UnstructuredText.Url(url)` and
`UnstructuredBinary.Inline(data, mimeType)` / `UnstructuredBinary.Url(url)`.

## Allowed languages and MIME types

The type parameter is the allow-list:

| Type                                                | Allows                         |
|-----------------------------------------------------|--------------------------------|
| `UnstructuredText[String]`                          | any language code, or none     |
| `UnstructuredText["en" \| "de"]`                    | `en`, `de`, or none            |
| `UnstructuredBinary[String]`                        | any MIME type, or none         |
| `UnstructuredBinary["image/png" \| "image/jpeg"]`   | `image/png`, `image/jpeg`, or none |

A string literal type or a union of string literal types is an allow-list; `String` means
unrestricted. The compiler rejects any other type argument. The literal type also types the inline
`languageCode` / `mimeType`, so `UnstructuredBinary.fromInline(bytes, "text/plain")` does not
compile for `UnstructuredBinary["image/png"]`.

The allow-list is part of the type, rather than an annotation, so it stays attached when the value
is nested in `Option`, `Either`, collections, or record fields, and so the compiler can emit the
static wire metadata for it. Decoding rejects an inline value whose language code or MIME type is
present but not in the allow-list; a missing language code or MIME type is always accepted.

## Schema

`UnstructuredText[L]` has the schema `variant { inline: text, url: url }` with the
`unstructured-text` role. `UnstructuredBinary[M]` has the schema `variant { inline: binary, url: url }`
with the `unstructured-binary` role. The allow-list becomes the `languages` restriction of the
`text` node or the `mime-types` restriction of the `binary` node. Decoding also accepts a bare
`text` / `binary` value as an inline value.

## Raw HTTP bodies

An HTTP endpoint that returns an unstructured value renders the inline content as the raw response
body instead of JSON. This also applies when the value is wrapped in `Option` or is on the `Right`
side of `Either`:

```scala
@agentDefinition(mount = "/files/{value}")
trait FilesAgent extends BaseAgent {
  class Id(val value: String)

  @endpoint(method = "GET", path = "/{name}")
  def file(name: String): Future[Either[String, Option[UnstructuredBinary[String]]]]
}
```

| Returned value                                    | HTTP response                                    |
|---------------------------------------------------|--------------------------------------------------|
| `Right(Some(UnstructuredBinary.Inline(data, m)))` | `200`, raw `data`, `Content-Type` from `m`       |
| `Right(None)`                                     | `404`                                            |
| `Left(error)`                                     | `500`, JSON error body                           |

`Option[UnstructuredText[L]]` and `Either[E, UnstructuredText[L]]` behave the same way.

Generated RPC clients copy agent method signatures as written and do not copy the imports of the
agent trait's file. When RPC client generation is enabled, write the fully qualified name in the
agent trait, for example `golem.schema.UnstructuredBinary[String]`.
