# Multimodal Helpers

The `golem.data` package provides building blocks for describing multimodal payloads; data structures that
combine different content types like text and binary segments with compile-time type constraints.

## Table of Contents

- [Core Concepts](#core-concepts)
- [Defining Content Constraints](#defining-content-constraints)
- [Building Multimodal Payloads](#building-multimodal-payloads)
- [Schema Derivation](#schema-derivation)
- [Complete Example](#complete-example)

---

## Core Concepts

### Content Types

| Type                        | Description                                              |
|-----------------------------|----------------------------------------------------------|
| `TextSegment[Lang]`         | Text content with language code constraints              |
| `BinarySegment[Descriptor]` | Binary content with MIME type constraints                |
| `Multimodal[A]`             | Wrapper that lifts a case class into a multimodal schema |

### Constraint Traits

| Trait                          | Purpose                                            |
|--------------------------------|----------------------------------------------------|
| `AllowedLanguages[Lang]`       | Defines permitted language codes for text segments |
| `AllowedMimeTypes[Descriptor]` | Defines permitted MIME types for binary segments   |

---

## Defining Content Constraints

Use enums with annotations to define the allowed content types at compile time.

### Language Constraints

```scala
import golem.runtime.annotations.languageCode
import golem.data.unstructured.AllowedLanguages

enum TranscriptLang:
  @languageCode("en")
  case English

  @languageCode("es")
  case Spanish

  @languageCode("fr")
  case French

object TranscriptLang:
  given AllowedLanguages[TranscriptLang] =
    golem.runtime.macros.AllowedLanguagesDerivation.derived
```

### MIME Type Constraints

```scala
import golem.runtime.annotations.mimeType
import golem.data.unstructured.AllowedMimeTypes

enum ImageMime:
  @mimeType("image/png")
  case Png

  @mimeType("image/jpeg")
  case Jpeg

  @mimeType("image/webp")
  case Webp

object ImageMime:
  given AllowedMimeTypes[ImageMime] =
    golem.runtime.macros.AllowedMimeTypesDerivation.derived
```

### Unconstrained Content

For content that accepts any language or MIME type, use the `Any` marker:

```scala
import golem.data.unstructured.{AllowedLanguages, AllowedMimeTypes}

// Accept any language
val text = TextSegment.inline[AllowedLanguages.Any]("Hello!", None)

// Accept any MIME type
val binary = BinarySegment.inline[AllowedMimeTypes.Any](bytes, "application/octet-stream")
```

---

## Building Multimodal Payloads

### Inline Content

Create segments with data embedded directly:

```scala
import golem.data.unstructured.{TextSegment, BinarySegment}

// Text with language code
val transcript = TextSegment.inline[TranscriptLang](
  text = "Hello, world!",
  languageCode = Some("en")
)

// Binary with MIME type
val image = BinarySegment.inline[ImageMime](
  bytes = imageBytes,
  mimeType = "image/png"
)
```

### URL References

For large content, reference external URLs:

```scala
// Text from URL
val remoteTranscript = TextSegment.url[TranscriptLang](
  "https://example.com/transcript.txt"
)

// Binary from URL
val remoteImage = BinarySegment.url[ImageMime](
  "https://example.com/image.png"
)
```

---

## Schema Derivation

### Wrapping with Multimodal

Use the `Multimodal` wrapper to convert a case class into a multimodal schema:

```scala
import golem.data.multimodal.Multimodal
import golem.data.unstructured.{TextSegment, BinarySegment}

// Define a bundle combining text and binary
final case class MediaBundle(
                              transcript: TextSegment[TranscriptLang],
                              image: BinarySegment[ImageMime]
                            )

// The multimodal type
type MediaPayload = Multimodal[MediaBundle]

// Create a payload
val payload = Multimodal(
  MediaBundle(
    transcript = TextSegment.inline[TranscriptLang]("Hello!", Some("en")),
    image = BinarySegment.inline[ImageMime](imageBytes, "image/png")
  )
)
```

### Automatic Schema Generation

Because everything is described via `GolemSchema`, the macro-generated agent metadata and RPC plans automatically
propagate modality descriptors to the host:

```scala
import golem.data.GolemSchema

// Schema is automatically derived
val schema: GolemSchema[MediaPayload] = summon[GolemSchema[MediaPayload]]

// The schema.schema field reveals the multimodal structure:
// Multimodal(List(
//   NamedElementSchema("transcript", UnstructuredText(Some(List("en", "es", "fr")))),
//   NamedElementSchema("image", UnstructuredBinary(Some(List("image/png", "image/jpeg", "image/webp"))))
// ))
```

---

## Complete Example

Here's a complete working example combining all the concepts:

```scala
import golem.data.GolemSchema
import golem.data.multimodal.Multimodal
import golem.data.unstructured.{AllowedLanguages, AllowedMimeTypes, BinarySegment, TextSegment}
import golem.runtime.annotations.{description, languageCode, mimeType}

// Define allowed languages
enum SupportedLang:
    @languageCode("en")
    case English
    @languageCode("de")
    case German

object SupportedLang:
    given AllowedLanguages[SupportedLang] =
      golem.runtime.macros.AllowedLanguagesDerivation.derived
    
// Define allowed MIME types
enum DocumentMime:
    @mimeType("application/pdf")
    case Pdf
    @mimeType("image/png")
    case Png

object DocumentMime:
    given AllowedMimeTypes[DocumentMime] =
      golem.runtime.macros.AllowedMimeTypesDerivation.derived

// Define the multimodal bundle
final case class DocumentPackage(summary: TextSegment[SupportedLang],
                                 document: BinarySegment[DocumentMime])

type DocumentPayload = Multimodal[DocumentPackage]

// Use in an agent trait
@description("Document processing agent")
trait DocumentAgent {
  @description("Process a document package")
  def process(input: DocumentPayload): TextSegment[SupportedLang]
}
```

 
