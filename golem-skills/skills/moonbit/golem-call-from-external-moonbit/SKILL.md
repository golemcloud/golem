---
name: golem-call-from-external-moonbit
description: "Calling Golem agents from external applications when the agent is written in MoonBit. Use when the user wants to invoke MoonBit agents from outside the Golem platform."
---

# Calling Agents from External Applications (MoonBit)

## Bridge Generation Not Yet Available

Golem's bridge SDK generation currently supports **TypeScript** and **Rust** target languages only. There is no MoonBit bridge generator yet.

## Alternative: Call the REST API from MoonBit

You can call any Golem agent using the REST API from a standalone MoonBit program compiled to native target. Use the `moonbitlang/async` package which provides an HTTP client.

### Setup

Create a separate MoonBit project (outside the Golem component) for the external CLI app:

```shell
moon new external-client
cd external-client
moon add moonbitlang/async
```

Set the preferred target to `native` in `moon.mod.json` since this is a standalone CLI app (not a WASM component):

```json
{
  "name": "external-client",
  "deps": {
    "moonbitlang/async": ">=0.18.0"
  },
  "preferred-target": "native"
}
```

In `moon.pkg` of the main package, import the HTTP module:

```json
{
  "is-main": true,
  "import": [
    "moonbitlang/async/http"
  ]
}
```

### Making Requests

Use `@http.post` to call the agent invocation endpoint:

```moonbit
async fn main {
  let body = @json.stringify(
    {
      "appName": "my-app",
      "envName": "local",
      "agentTypeName": "MyAgent",
      "parameters": { "type": "Tuple", "elements": [{ "type": "ComponentModel", "value": "my-instance" }] },
      "methodName": "doSomething",
      "methodParameters": { "type": "Tuple", "elements": [{ "type": "ComponentModel", "value": "input" }] },
      "mode": "await"
    },
  )
  let (response, data) = @http.post(
    "http://localhost:9881/v1/agents/invoke-agent",
    body.to_bytes(),
    headers={
      "Content-Type": "application/json",
      "Authorization": "Bearer 5c832d93-ff85-4a8f-9803-513950fdfdb1",
    },
  )
  println(response.code)
  println(data.text())
}
```

### Building and Running

```shell
moon build --target native
moon run --target native src/main
```

### REST API Reference

**Endpoint**: `POST /v1/agents/invoke-agent`

**Request body**:

```json
{
  "appName": "my-app",
  "envName": "local",
  "agentTypeName": "MyAgent",
  "parameters": {
    "type": "Tuple",
    "elements": [
      { "type": "ComponentModel", "value": "my-instance" }
    ]
  },
  "methodName": "doSomething",
  "methodParameters": {
    "type": "Tuple",
    "elements": [
      { "type": "ComponentModel", "value": "input" }
    ]
  },
  "mode": "await"
}
```

**Response body** (when mode is `"await"`):

```json
{
  "result": {
    "type": "Tuple",
    "elements": [
      { "type": "ComponentModel", "value": <result_value> }
    ]
  }
}
```

### Authentication

- **Local server**: Use bearer token `5c832d93-ff85-4a8f-9803-513950fdfdb1`
- **Golem Cloud**: Use your API token
- **Custom deployment**: Use the configured bearer token

## Using a Generated TypeScript or Rust Bridge

If you need a typed client, you can generate a **TypeScript** or **Rust** bridge SDK even for agents written in MoonBit. The bridge target language is independent of the agent's source language:

```yaml
bridge:
  ts:
    agents: "*"
  rust:
    agents: "*"
```

Then use the generated TypeScript or Rust client from your external application. See the `golem-call-from-external-ts` or `golem-call-from-external-rust` skills (available in TypeScript and Rust project templates) for details.
