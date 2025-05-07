# OpenAI Embedding Provider for Golem

This component provides OpenAI embedding functionality for Golem applications through the WIT interface.

## Features

- Generate embeddings using OpenAI's embedding models
- Configurable model selection
- Error handling for rate limits and other API issues

## Prerequisites

- Rust toolchain with cargo
- OpenAI API key
- Golem runtime

## Setup

2. Set your OpenAI API key using one of these methods:

   a. Create a `.env` file (recommended):
   ```bash
   # Copy the template file
   cp .env.template .env
   # Edit the .env file and add your API key
   ```

   b. Or set it as an environment variable:
   ```bash
   export OPENAI_API_KEY="your-api-key-here"
   ```

## Deployment

Run the deployment script to build and install the component:

```bash
./deploy.sh
```

This will:
1. Build the WASM component
2. Install it to `~/.golem/components/embed_openai.wasm`

## Usage

Once deployed, you can use the embedding component in your Golem applications by referencing it in your WIT configuration.

Example:

```rust
// Import the embedding interface
use golem::embed::embed;

// Generate embeddings
let embeddings = embed::generate(vec![embed::ContentPart::Text("Your text here".to_string())], embed::Config::default());
```

## Configuration

You can customize the embedding behavior using the `Config` object:

- `model`: Specify which OpenAI embedding model to use (defaults to "text-embedding-3-large")
- `user`: Optional user identifier for OpenAI API
- `output_format`: Must be `OutputFormat::FloatArray`
- `output_dtype`: Must be `OutputDtype::F32`

## Limitations

- Only supports text embeddings (image embeddings not supported)
- Reranking functionality is not implemented
- Only supports float array output format and F32 data type