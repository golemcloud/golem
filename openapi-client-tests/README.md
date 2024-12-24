# OpenAPI Client Tests

This repository contains test cases for validating clients generated using OpenAPI specifications. The tests cover multiple client languages, including TypeScript, Python, and Rust.

## Project Structure

```plaintext
openapi-client-tests/
├── typescript-client/
│   ├── test.ts
│   ├── package.json
│   ├── tsconfig.json
├── python-client/
│   ├── test_api.py
│   ├── requirements.txt
├── rust-client/
│   ├── examples/
│   │   ├── test.rs
│   ├── Cargo.toml
├── README.md
```

## Prerequisites

- Node.js (for TypeScript client testing)
- Python 3.9+ (for Python client testing)
- Rust (for Rust client testing)

## How to Run Tests

### TypeScript Client

1. Navigate to the TypeScript client directory:
   ```bash
   cd typescript-client
   ```

2. Install dependencies:
   ```bash
   npm install
   ```

3. Run the test:
   ```bash
   npm run test
   ```

### Python Client

1. Navigate to the Python client directory:
   ```bash
   cd python-client
   ```

2. Install dependencies:
   ```bash
   pip install -r requirements.txt
   ```

3. Run the test:
   ```bash
   python -m unittest test_api.py
   ```

### Rust Client

1. Navigate to the Rust client directory:
   ```bash
   cd rust-client
   ```

2. Build and run the example:
   ```bash
   cargo run --example test
   ```

## License

This project is licensed under the [Apache-2.0](../LICENSE).