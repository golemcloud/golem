# Embedding Providers Implementation Plan

## Overview

This project involves implementing WASM components for four embedding providers (OpenAI, Cohere, Hugging Face, and Voyage AI) following the `golem:embed@1.0.0` WIT interface. Each implementation will include custom durability semantics using the Golem durability API.

## Project Structure

The project is organized as a Rust workspace with the following structure:

```
embed-providers/
├── Cargo.toml (workspace)
├── wit/
│   └── embed.wit (WIT interface definition)
├── crates/
│   ├── embed-common/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs (common traits and utilities)
│   │       ├── durability.rs (durability operations)
│   │       └── wit.rs (WIT interface implementation helpers)
│   ├── embed-openai/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs (OpenAI client implementation)
│   │       └── wit.rs (WIT interface binding)
│   ├── embed-cohere/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs (Cohere client implementation)
│   │       └── wit.rs (WIT interface binding)
│   ├── embed-huggingface/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs (Hugging Face client implementation)
│   │       └── wit.rs (WIT interface binding)
│   └── embed-voyageai/
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs (Voyage AI client implementation)
│           └── wit.rs (WIT interface binding)
└── build.sh (build script for all components)
```

## Implementation Approach

### Common Code (embed-common)

The `embed-common` crate provides shared functionality:

1. **EmbeddingProvider Trait**: Defines the interface that all providers must implement
2. **Durability Operations**: Structures for embedding and reranking operations with durability support
3. **Error Handling**: Common error types and conversion utilities
4. **WIT Interface Helpers**: Functions to convert between provider-specific and WIT-defined types

### Provider-Specific Implementations

Each provider implementation will:

1. Implement the `EmbeddingProvider` trait
2. Handle provider-specific API details and authentication
3. Implement custom durability for embedding operations
4. Provide proper error handling and conversion
5. Include comprehensive tests

### Custom Durability Implementation

The durability implementation will:

1. Use Golem's durability API to persist embedding operations
2. Implement operation-level durability (not just raw HTTP requests)
3. Support both embedding and reranking operations
4. Allow for operation replay in case of failures

## Configuration

All components will be configured using environment variables:

- `OPENAI_API_KEY`: API key for OpenAI
- `COHERE_API_KEY`: API key for Cohere
- `HUGGINGFACE_API_KEY`: API key for Hugging Face
- `VOYAGEAI_API_KEY`: API key for Voyage AI

Additional configuration options will be passed through the WIT interface's `config` parameter.

## Testing Strategy

1. **Unit Tests**: Test individual provider implementations
2. **Integration Tests**: Test the WIT interface implementation
3. **Durability Tests**: Verify durability operations work correctly
4. **Golem Compatibility**: Test components within Golem 1.2.x

## Build Process

The build process will:

1. Compile each component to a WASM module using `cargo component`
2. Ensure compatibility with WASI 0.2
3. Output the following files:
   - `embed-openai.wasm`
   - `embed-cohere.wasm`
   - `embed-huggingface.wasm`
   - `embed-voyageai.wasm`

## Implementation Details

### OpenAI Implementation

- Support for text-embedding-3-large model
- Handle rate limiting and error responses
- Implement custom durability for embedding operations

### Cohere Implementation

- Support for Cohere's embedding models
- Implement both embedding and reranking functionality
- Handle Cohere-specific API responses

### Hugging Face Implementation

- Support for Hugging Face's embedding models
- Handle authentication and API specifics
- Support for model selection

### Voyage AI Implementation

- Support for Voyage AI's embedding models and rerankers
- Implement custom durability
- Handle Voyage AI-specific API details

## Deliverables

1. Four WASM components (WASI 0.2 compatible):
   - `embed-openai.wasm`
   - `embed-cohere.wasm`
   - `embed-huggingface.wasm`
   - `embed-voyageai.wasm`
2. Comprehensive test suite for each component
3. Documentation on usage and configuration
4. Build scripts and CI integration

## Next Steps

1. Complete the implementation of the `embed-common` crate
2. Implement each provider-specific crate
3. Create build scripts and CI integration
4. Test all components within Golem 1.2.x
5. Document usage and configuration