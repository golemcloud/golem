# Prompts

Every SDK ships the default prompts and output schemas in `fixtures/prompts-v1.json`. A constructor or
an ordinary config value can replace the prompt text. A Memory stores the prompt identity, and the
embedder identity, with the data it derives.

| Prompt | Identity | Inputs | Output |
| --- | --- | --- | --- |
| Summarizer | `golem-memory/summarizer/v1` | the observation parts | `{ "summary": string }` |
| Extractor | `golem-memory/extractor/v1` | the observation parts | entities with aliases and assertions with cited part IDs and optional validity |
| Arbiter | `golem-memory/arbiter/v1` | the query and the nonempty member candidates | `{ "member": string }` |

## Rules

- Every prompt treats rendered content as data, not instructions. A prompt produces no executable
  query, and its model call uses no tools.
- The summarizer compresses one observation and adds no facts. An empty summary is a successful empty
  derivation.
- The extractor cites part IDs for every assertion. Assertion subjects and objects are entity IDs of
  the same output, and entity IDs are unique. It fills validity only from times that the observation
  states. Two entities are not merged only because they look similar.
- The arbiter prefers the candidate with more direct supporting evidence for the query, breaks ties in
  member order, and merges or rewrites nothing.
- Output that does not match the schema, or that breaks one of these rules, is malformed. Malformed
  summarizer or extractor output is a failed derivation. Malformed arbiter output is an `arbiter`
  error. An arbiter selection that names no rendered candidate is an `invalid-selection` error.
- A valid citation does not prove a claim. A Memory keeps model provenance and the original evidence.

## Rendering

The `{{parts}}`, `{{query}}` and `{{candidates}}` placeholders are rendered as the `rendering` section of
`prompts-v1.json` describes. Each part becomes `<part id="pN">TEXT</part>`. Each member with a nonempty
result becomes a `<candidate member="NAME">` block with one `<item>` per item, in member order.
Rendered content escapes `&`, `<` and `>`, so observed content cannot close or open a tag.

`renderingCases` fix the exact rendering. `outputCases` fix the classification of valid, malformed and
invalid outputs.
