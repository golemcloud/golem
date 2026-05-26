---
title: "Golem 1.5 features — Part 3: MCP"
date: "2026-04-11T00:00:00Z"
author: "Daniel Vigovszky"
tags: ["Product Updates", "Engineering Articles"]
slug: "golem-1-5-features-part-3-mcp"
originalUrl: "https://blog.vigoo.dev/posts/golem15-part3-mcp/"
---

## Introduction

This post showcases new features in **Golem 1.5**, released at the end of April 2026. It assumes familiarity with Golem and is part of a series of short posts covering individual features. Check the [other Golem-related posts](https://blog.vigoo.dev/tags/golem/) for more information.

## MCP

MCP (Model Context Protocol) became a standard way to connect AI applications. With Golem 1.5 it can now be automatically exposed through any Golem application without additional code — it just needs to be enabled in the application manifest.

### Enabling

The manifest includes an `mcp` section to deploy agents to specific subdomains per environment:

```yaml
mcp:
  deployments:
    local:
      - domain: mcp-demo.localhost:9007
        agents:
          CounterAgent: {}
```

### Security

OAuth protection can be attached to MCP deployments by configuring a security scheme using the `golem` CLI with an OAuth provider:

```yaml
mcp:
  deployments:
    local:
      - domain: mcp-demo.localhost:9007
        agents:
          CounterAgent:
            securityScheme: mcp-oauth
```

Setting up a mock OAuth2 server and registering a custom security scheme:

```bash
docker run -d \
  --name "golem-mock-oauth2" \
  -p "9099:8080" \
  ghcr.io/navikt/mock-oauth2-server:2.1.10

CLIENT_ID="golem-mcp-client"
CLIENT_SECRET="golem-mcp-secret"
REDIRECT_URL="http://mcp-demo.localhost:9007/mcp/oauth/callback"

golem -L api security-scheme create \
  --provider-type custom \
  --custom-provider-name "mock-oauth2" \
  --custom-issuer-url "http://localhost:9099/golem" \
  --client-id "${CLIENT_ID}" \
  --client-secret "${CLIENT_SECRET}" \
  --scope openid \
  --scope email \
  --scope profile \
  --redirect-url "${REDIRECT_URL}" \
  mcp-oauth
```

### Demo

A counter agent example demonstrates automatic MCP tool mapping:

```rust
#[agent_definition(mount = "/counters/{name}")]
pub trait CounterAgent {
    // The agent constructor, its parameters identify the agent
    fn new(name: String) -> Self;

    #[description("Increment by a given number")]
    fn increment_by(&mut self, n: u32) -> u32;
}

struct CounterImpl {
    _name: String,
    count: u32,
}

#[agent_implementation]
impl CounterAgent for CounterImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            count: 0,
        }
    }

    fn increment_by(&mut self, n: u32) -> u32 {
        self.count += n;
        self.count
    }
}
```

You can explore the exposed MCP surface using the MCP Inspector:

```bash
npx @modelcontextprotocol/inspector node build/index.js
```

### Mapping

Agent methods map to MCP entities based on parameters:

- Singleton, no parameters → Resource
- Non-singleton, no parameters → Resource template
- Any with parameters → Tool

### Metadata

Descriptions and prompts can be attached to agents and methods for MCP metadata.

```typescript
@description("Increments the counter by the number provided in the `n` parameter")
@prompt("Increment by a given number")
async increment_by(n: number): Promise<number> {
  // ...
}
```

```rust
#[description("Increments the counter by the number provided in the `n` parameter")]
#[prompt("Increment by a given number")]
fn increment_by(&mut self, n: u32) -> u32;
```

```scala
@description("Increments the counter by the number provided in the `n` parameter")
@prompt("Increment by a given number")
def incrementBy(n: Int): Future[Int]
```

```moonbit
///| Increments the counter by the number provided in the `n` parameter
#derive.prompt_hint("Increment by a given number")
pub fn Counter::increment(self : Self, n : UInt32) -> UInt32 {
```

### Special Data Types

Three special data types support MCP exposure: unstructured text (with optional language codes), unstructured binary (with optional MIME types), and multimodal types for flexible input handling across text, binary, and structured data formats.

#### Unstructured text

```typescript
myMethod(
  anyText: UnstructuredText,
  constrainedText: UnstructuredText<['en', 'de']>
) {
  // ...
}
```

```rust
#[derive(AllowedLanguages)]
enum MyLangs { En, #[code("de")] German }

fn my_method(
    &self,
    any_text: UnstructuredText,
    constrained_text: UnstructuredText<MyLangs>
);
```

```scala
def myMethod(
  anyText: TextSegment[AllowedLanguages.Any],
  constrainedText: TextSegment[MyLangs]
)

sealed trait MyLangs
object MyLangs {
  case object En extends MyLangs

  @golem.runtime.annotations.languageCode("de")
  case object German extends MyLangs

  implicit val allowed: AllowedLanguages[MyLangs] =
    golem.runtime.macros.AllowedLanguagesDerivation.derived
}
```

```moonbit
#derive.text_languages("constrained_text", "en", "de")
pub fn MyAgent::my_method(
  self : Self,
  any_text : UnstructuredText,
  constrained_text : UnstructuredText,
) -> Unit {
  ...
}
```

#### Unstructured binary

```typescript
myMethod(
  anyBinary: UnstructuredBinary,
  image: UnstructuredBinary<['image/png', 'image/jpeg']>
) {
  // ...
}
```

```rust
#[derive(Debug, Clone, AllowedMimeTypes)]
enum Image {
    #[mime_type("image/png")]
    Png,
    #[mime_type("image/jpeg")]
    Jpeg,
}

fn my_method(
    &self,
    any_binary: UnstructuredBinary,
    image: UnstructuredBinary<Image>
);
```

```scala
def myMethod(
  anyBinary: BinarySegment[AllowedMimeTypes.Any],
  image: BinarySegment[Image]
)

sealed trait Image
object Image {
  @golem.runtime.annotations.mimeType("image/png")
  case object Png extends Image
  @golem.runtime.annotations.mimeType("image/jpeg")
  case object Jpeg extends Image

  implicit val allowed: AllowedMimeTypes[Image] =
    golem.runtime.macros.AllowedMimeTypesDerivation.derived
}
```

```moonbit
#derive.mime_types("image", "image/png", "image/jpeg")
pub fn MyAgent::my_method(
  self : Self,
  any_binary : UnstructuredBinary,
  image : UnstructuredBinary,
) -> Unit {
  ...
}
```

#### Multimodal (basic)

```typescript
textOrBinary(input: Multimodal) { ... }
```

```rust
fn text_or_binary(&self, input: Multimodal) -> Multimodal { input }
```

```scala
def textOrBinary(input: MultimodalItems.Basic): Future[MultimodalItems.Basic]
```

```moonbit
pub fn MyAgent::text_or_binary(
  self : Self,
  input : @types.Multimodal[TextOrBinary],
) -> String { ... }
```

#### Multimodal with custom type

```typescript
type MyStructuredType = { ...}
textOrBinaryOrStructured(input: MultimodalCustom<MyStructuredType>) { ... }
```

```rust
#[derive(Schema)]
struct MyStructuredType { /* ... */ }

fn text_or_binary_or_structured(
    &self,
    input: MultimodalCustom<MyStructuredType>,
) -> MultimodalCustom<MyStructuredType> { input }
```

```scala
final case class MyStructuredType(/* ... */)
object MyStructuredType { implicit val schema: Schema[MyStructuredType] = Schema.derived }

def textOrBinaryOrStructured(
  input: MultimodalItems.WithCustom[MyStructuredType]
): Future[MultimodalItems.WithCustom[MyStructuredType]]
```

```moonbit
#derive.golem_schema
pub(all) struct MyStructuredType { /* ... */ }

pub fn MyAgent::text_or_binary_or_structured(
  self : Self,
  input : @types.Multimodal[CustomModality[MyStructuredType]],
) -> String { ... }
```

#### Fully custom multimodal

```typescript
export type TextOrImage =
  | { tag: 'text'; val: UnstructuredText<['en', 'de'>] }
  | { tag: 'image'; val: UnstructuredBinary<['image/jpeg', 'image/png']> };
fullyCustom(input: MultimodalAdvanced<TextOrImage>) { ... }
```

```rust
#[derive(AllowedLanguages)]
enum TextLang { En, #[code("de")] German }

#[derive(AllowedMimeTypes)]
enum ImageType {
    #[mime_type("image/jpeg")] Jpeg,
    #[mime_type("image/png")] Png,
}

#[derive(Schema, MultimodalSchema)]
enum TextOrImage {
    Text(UnstructuredText<TextLang>),
    Image(UnstructuredBinary<ImageType>),
}

fn fully_custom(
    &self,
    input: MultimodalAdvanced<TextOrImage>,
) -> MultimodalAdvanced<TextOrImage> { input }
```

```scala
sealed trait TextLang
object TextLang {
  @golem.runtime.annotations.languageCode("en")
  case object En extends TextLang
  @golem.runtime.annotations.languageCode("de")
  case object De extends TextLang
  implicit val allowed: AllowedLanguages[TextLang] =
    golem.runtime.macros.AllowedLanguagesDerivation.derived
}

sealed trait ImageType
object ImageType {
  @golem.runtime.annotations.mimeType("image/jpeg")
  case object Jpeg extends ImageType
  @golem.runtime.annotations.mimeType("image/png")
  case object Png extends ImageType
  implicit val allowed: AllowedMimeTypes[ImageType] =
    golem.runtime.macros.AllowedMimeTypesDerivation.derived
}

final case class TextOrImage(
  text: TextSegment[TextLang],
  image: BinarySegment[ImageType],
)
object TextOrImage { implicit val schema: GolemSchema[TextOrImage] = /* derived */ }

def fullyCustom(input: Multimodal[TextOrImage]): Future[Multimodal[TextOrImage]]
```

```moonbit
#derive.multimodal
pub(all) enum TextOrImage {
  Text(UnstructuredText)
  Image(UnstructuredBinary)
}

#derive.text_languages("input.Text", "en", "de")
#derive.mime_types("input.Image", "image/jpeg", "image/png")
pub fn MyAgent::fully_custom(
  self : Self,
  input : @types.Multimodal[TextOrImage],
) -> String { ... }
```
