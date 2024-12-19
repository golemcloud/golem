# Manual Testing Guide for Golem Worker Gateway OpenAPI Features

## Prerequisites

1. Running Golem Worker Gateway
2. Postman or similar API testing tool
3. Web browser for Swagger UI testing

## Test Scenarios

### 1. Complex API Definition Testing

#### 1.1 Complex Types
- Test nested objects with multiple levels
- Test arrays of complex objects
- Test optional fields
- Test enum types
- Test map types with various value types
- Test date/time fields
- Test binary data fields

#### 1.2 Path Parameters
- Test single path parameters
- Test multiple path parameters
- Test path parameters with special characters
- Test optional path parameters
- Test wildcard path parameters

#### 1.3 Query Parameters
- Test required vs optional parameters
- Test array query parameters
- Test object query parameters
- Test boolean parameters
- Test number parameters
- Test date parameters

### 2. Security Testing

#### 2.1 Authentication
- Test API key authentication
- Test Bearer token authentication
- Test OAuth2 flows
- Test multiple authentication methods

#### 2.2 CORS
- Test allowed origins
- Test allowed methods
- Test allowed headers
- Test credentials handling
- Test preflight requests
- Test complex CORS scenarios

### 3. Swagger UI Testing

#### 3.1 Basic Functionality
- Verify Swagger UI loads correctly
- Test theme switching
- Test API endpoint exploration
- Test "Try it out" functionality
- Test response visualization
- Test schema visualization

#### 3.2 Advanced Features
- Test authorization persistence
- Test request history
- Test response examples
- Test model examples
- Test filter functionality
- Test deep linking

### 4. Client Library Testing

#### 4.1 TypeScript Client
```typescript
// Test complex request
const request = {
    string_field: "test",
    optional_field: 42,
    array_field: ["a", "b", "c"],
    nested_field: {
        field1: "nested",
        field2: 123,
        field3: ["test", null, "value"]
    },
    enum_field: "Variant1",
    map_field: { key1: 1, key2: 2 }
};

// Verify response
const response = await api.createComplex(request);
assert(response.id !== undefined);
assert(response.data.string_field === request.string_field);
```

#### 4.2 Python Client
```python
# Test complex request
request = ComplexRequest(
    string_field="test",
    optional_field=42,
    array_field=["a", "b", "c"],
    nested_field=NestedObject(
        field1="nested",
        field2=123,
        field3=["test", None, "value"]
    ),
    enum_field="Variant1",
    map_field={"key1": 1, "key2": 2}
)

# Verify response
response = api.create_complex(request)
assert response.id is not None
assert response.data.string_field == request.string_field
```

### 5. Error Handling

#### 5.1 Validation Errors
- Test invalid request bodies
- Test invalid path parameters
- Test invalid query parameters
- Test missing required fields
- Test invalid field types

#### 5.2 HTTP Errors
- Test 400 Bad Request scenarios
- Test 401 Unauthorized scenarios
- Test 403 Forbidden scenarios
- Test 404 Not Found scenarios
- Test 429 Rate Limit scenarios
- Test 500 Internal Server Error scenarios

### 6. Performance Testing

#### 6.1 Load Testing
- Test with 100 concurrent requests
- Test with sustained load (10 RPS for 5 minutes)
- Test with burst traffic patterns
- Monitor response times
- Monitor error rates

#### 6.2 Resource Usage
- Monitor memory usage
- Monitor CPU usage
- Monitor disk I/O
- Monitor network I/O

## Common Issues

1. Type Conversion
   - Check for any loss of precision in number types
   - Verify complex type structures maintain relationships
   - Ensure optional fields are handled correctly

2. Security
   - Verify token passing works
   - Check CORS headers in responses
   - Test error responses for auth failures

3. Path Parameters
   - Verify URL encoding/decoding
   - Test multi-segment parameter limits
   - Check parameter validation

## Reporting Issues

When reporting issues, include:
1. API Definition used
2. Generated OpenAPI spec
3. Steps to reproduce
4. Expected vs actual behavior
5. Client library details (if applicable)

## Test Environment Setup

1. Install dependencies:
```bash
npm install -g @openapitools/openapi-generator-cli
pip install openapi-generator-cli
```

2. Generate test certificates:
```bash
openssl req -x509 -newkey rsa:4096 -keyout key.pem -out cert.pem -days 365 -nodes
```

3. Configure test environment:
```bash
export TEST_API_KEY=test-key
export TEST_JWT_SECRET=test-secret
```

## Running Tests

1. Start the test server:
```bash
cargo run --bin test-server
```

2. Generate client libraries:
```bash
openapi-generator-cli generate -i openapi.json -g typescript-fetch -o ./ts-client
openapi-generator-cli generate -i openapi.json -g python -o ./python-client
```

3. Run client tests:
```bash
cd ts-client && npm test
cd python-client && python -m pytest
```
