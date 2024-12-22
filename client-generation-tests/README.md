# Client Generation Tests

This directory contains tests for generating and validating API clients using OpenAPI Generator.

## Prerequisites

1. Python 3.8 or higher
2. Node.js and npm (for TypeScript tests)
3. OpenAPI Generator CLI
4. Java Runtime Environment (JRE) for OpenAPI Generator

## Setup

1. Install Python dependencies:
```bash
pip install -r requirements.txt
```

2. Install OpenAPI Generator CLI:
```bash
npm install @openapitools/openapi-generator-cli -g
```

3. Install TypeScript dependencies (for TypeScript tests):
```bash
npm install -g ts-node typescript @types/node
```

## Running Tests

To run all tests:
```bash
pytest tests/ -v
```

To run specific test:
```bash
pytest tests/test_client_generation.py -v -k test_python_client
pytest tests/test_client_generation.py -v -k test_typescript_client
```

## Test Structure

- `test_server.py`: FastAPI-based test server implementation
- `test_client_generation.py`: Test cases for client generation and validation

## Test Cases

1. Python Client Test:
   - Generates Python client from OpenAPI spec
   - Tests CRUD operations using the generated client

2. TypeScript Client Test:
   - Generates TypeScript client from OpenAPI spec
   - Tests CRUD operations using the generated client 