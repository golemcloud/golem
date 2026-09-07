---
name: golem-agent-reflection
description: "Choosing Golem agent reflection levels and identity lookup behavior across SDKs. Use when agent types or methods are discovered dynamically, schemas are inspected at runtime, or a full AgentId must be resolved."
---

# Agent Reflection

Use the narrowest client surface that matches what the caller knows:

- Use a generated or definition-owned client when the target type and methods are known in source.
- Use a caller-owned contract when the target implementation is not imported but its identity and method schemas are known.
- Use runtime reflection when the type or method is selected dynamically and the caller needs registered constructor, input, or output schemas.
- Use a schema-free dynamic client only when infrastructure deliberately works with schema-native values and arbitrary method names.

Agent identity strings are environment-scoped. A full `AgentId` pairs that string with a component ID so it can cross component boundaries without becoming globally ambiguous.

Discovery lookups are optional: a name or full-ID lookup returns no type when the deployment is missing, the identity is malformed, or the caller cannot view it. Parsing an identity is strict and reports malformed input. Full-ID discovery never creates the target agent.

Reflected schema graphs are immutable snapshots of the deployed contract. Validate or pack JSON through the reflected constructor or method schema, and treat a missing or malformed declared output as a remote output error.

Load the language-specific reflection skill for concrete SDK APIs and examples.
