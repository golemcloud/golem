---
name: golem-make-http-request-moonbit
description: "Making outgoing HTTP requests from MoonBit Golem agent code. Use when the user asks to call an external API, make HTTP requests, or fetch data from a URL."
---

# Making Outgoing HTTP Requests (MoonBit)

## Overview

MoonBit Golem agents use the SDK's `@http` package (`golemcloud/golem_sdk/http`) for outgoing HTTP requests. This package re-exports the WASI HTTP types and the outgoing handler function.

All outgoing HTTP requests made from a Golem agent are **automatically durably persisted** — Golem records the request and response in the oplog, so on replay the response is read from the log rather than re-executing the network call.

## Setup

Add the HTTP package to your agent's `moon.pkg`:

```
import {
  "golemcloud/golem_sdk/http" @http,
}
```

No WIT changes or binding regeneration is needed — the SDK already includes the HTTP imports.

## GET Request

```moonbit
fn make_get_request(url_authority : String, path : String) -> String {
  // 1. Create headers
  let headers = @http.Fields::new()

  // 2. Create outgoing request (defaults to GET)
  let request = @http.OutgoingRequest::new(headers)
  let _ = request.set_scheme(Some(@http.Https))
  let _ = request.set_authority(Some(url_authority))
  let _ = request.set_path_with_query(Some(path))

  // 3. Finish the body (empty for GET)
  let body = request.body().unwrap()
  @http.OutgoingBody::finish(body, None).unwrap()

  // 4. Send the request
  let future_response = @http.handle(request, None).unwrap()

  // 5. Wait for the response
  let pollable = future_response.subscribe()
  pollable.block()
  let response = future_response.get().unwrap().unwrap().unwrap()

  // 6. Read status
  let status = response.status()

  // 7. Read response body
  let incoming_body = response.consume().unwrap()
  let stream = incoming_body.stream().unwrap()
  let bytes = stream.blocking_read(1048576UL).unwrap()  // read up to 1MB
  stream.drop()
  @http.IncomingBody::finish(incoming_body)

  let body_str = String::from_utf8_lossy(bytes)
  body_str
}
```

## POST with JSON Body

```moonbit
fn make_post_request(
  authority : String,
  path : String,
  json_body : String
) -> (UInt, String) {
  // 1. Create headers with Content-Type
  let headers = @http.Fields::from_list(
    [
      ("Content-Type", b"application/json"),
      ("Accept", b"application/json"),
    ],
  ).unwrap()

  // 2. Create request and set method to POST
  let request = @http.OutgoingRequest::new(headers)
  let _ = request.set_method(@http.Post)
  let _ = request.set_scheme(Some(@http.Https))
  let _ = request.set_authority(Some(authority))
  let _ = request.set_path_with_query(Some(path))

  // 3. Write the request body
  let body = request.body().unwrap()
  let output_stream = body.write().unwrap()
  let body_bytes = json_body.to_utf8_bytes()
  output_stream.blocking_write_and_flush(body_bytes).unwrap()
  output_stream.drop()  // must drop stream before finishing body
  @http.OutgoingBody::finish(body, None).unwrap()

  // 4. Send and wait
  let future_response = @http.handle(request, None).unwrap()
  let pollable = future_response.subscribe()
  pollable.block()
  let response = future_response.get().unwrap().unwrap().unwrap()

  // 5. Read response
  let status = response.status()
  let incoming_body = response.consume().unwrap()
  let stream = incoming_body.stream().unwrap()
  let bytes = stream.blocking_read(1048576UL).unwrap()
  stream.drop()
  @http.IncomingBody::finish(incoming_body)

  (status, String::from_utf8_lossy(bytes))
}
```

## Setting Headers

Headers are `Fields` resources. Field values are `FixedArray[Byte]` (the WASI `field-value` type):

```moonbit
// From a list of (name, value) pairs
let headers = @http.Fields::from_list(
  [
    ("Authorization", b"Bearer my-token"),
    ("Accept", b"application/json"),
    ("X-Custom-Header", b"custom-value"),
  ],
).unwrap()

// Or construct empty and append
let headers = @http.Fields::new()
let _ = headers.append("Authorization", b"Bearer my-token")
let _ = headers.append("Content-Type", b"application/json")
```

## Reading Response Headers

```moonbit
let response = future_response.get().unwrap().unwrap().unwrap()

// Status code (UInt)
let status = response.status()

// Response headers (immutable Fields)
let resp_headers = response.headers()
let content_type_values = resp_headers.get("Content-Type")
// content_type_values : Array[FixedArray[Byte]]
```

## Setting Timeouts

Use `RequestOptions` to configure transport-level timeouts:

```moonbit
let options = @http.RequestOptions::new()
let _ = options.set_connect_timeout(Some(5_000_000_000UL))         // 5 seconds in nanoseconds
let _ = options.set_first_byte_timeout(Some(10_000_000_000UL))     // 10 seconds
let _ = options.set_between_bytes_timeout(Some(30_000_000_000UL))  // 30 seconds

let future_response = @http.handle(request, Some(options)).unwrap()
```

## Error Handling

```moonbit
fn fetch_data(authority : String, path : String) -> Result[String, String] {
  let headers = @http.Fields::new()
  let request = @http.OutgoingRequest::new(headers)
  let _ = request.set_scheme(Some(@http.Https))
  let _ = request.set_authority(Some(authority))
  let _ = request.set_path_with_query(Some(path))

  let body = request.body().unwrap()
  @http.OutgoingBody::finish(body, None).unwrap()

  match @http.handle(request, None) {
    Err(error_code) => Err("Request failed: " + error_code.to_string())
    Ok(future_response) => {
      let pollable = future_response.subscribe()
      pollable.block()
      match future_response.get() {
        Some(Ok(Ok(response))) => {
          let status = response.status()
          let incoming_body = response.consume().unwrap()
          let stream = incoming_body.stream().unwrap()
          let bytes = stream.blocking_read(1048576UL).unwrap()
          stream.drop()
          @http.IncomingBody::finish(incoming_body)
          if status >= 200 && status < 300 {
            Ok(String::from_utf8_lossy(bytes))
          } else {
            Err(
              "HTTP " + status.to_string() + ": " +
              String::from_utf8_lossy(bytes),
            )
          }
        }
        Some(Ok(Err(error_code))) =>
          Err("HTTP error: " + error_code.to_string())
        Some(Err(_)) => Err("Response already consumed")
        None => Err("Response not ready")
      }
    }
  }
}
```

## Reading Large Response Bodies

The `blocking_read` call returns up to the requested number of bytes. For larger responses, read in a loop:

```moonbit
fn read_full_body(incoming_body : @http.IncomingBody) -> FixedArray[Byte] {
  let stream = incoming_body.stream().unwrap()
  let chunks : Array[FixedArray[Byte]] = []
  loop {
    match stream.blocking_read(65536UL) {
      Ok(chunk) => {
        if chunk.length() == 0 {
          break
        }
        chunks.push(chunk)
      }
      Err(@http.StreamError::Closed) => break
      Err(e) => panic()
    }
  }
  stream.drop()
  @http.IncomingBody::finish(incoming_body)
  // Concatenate chunks
  let total = chunks.fold(init=0, fn(acc, c) { acc + c.length() })
  let result = FixedArray::make(total, b'\x00')
  let mut offset = 0
  for chunk in chunks {
    chunk.blit_to(result, len=chunk.length(), src_offset=0, dst_offset=offset)
    offset += chunk.length()
  }
  result
}
```

## Complete Example in an Agent

```moonbit
/// An agent that fetches data from an external API
#derive.agent
pub(all) struct DataFetcher {
  base_url : String
  mut last_result : String
}

///|
fn DataFetcher::new(base_url : String) -> DataFetcher {
  { base_url, last_result: "" }
}

///|
/// Fetch data from the configured API endpoint
pub fn DataFetcher::fetch(self : Self, path : String) -> String {
  let headers = @http.Fields::from_list(
    [("Accept", b"application/json")],
  ).unwrap()

  let request = @http.OutgoingRequest::new(headers)
  let _ = request.set_scheme(Some(@http.Https))
  let _ = request.set_authority(Some(self.base_url))
  let _ = request.set_path_with_query(Some(path))

  let body = request.body().unwrap()
  @http.OutgoingBody::finish(body, None).unwrap()

  let future_response = @http.handle(request, None).unwrap()
  let pollable = future_response.subscribe()
  pollable.block()
  let response = future_response.get().unwrap().unwrap().unwrap()

  let incoming_body = response.consume().unwrap()
  let stream = incoming_body.stream().unwrap()
  let bytes = stream.blocking_read(1048576UL).unwrap()
  stream.drop()
  @http.IncomingBody::finish(incoming_body)

  let result = String::from_utf8_lossy(bytes)
  self.last_result = result
  result
}
```

## Calling Golem Agent HTTP Endpoints

When making HTTP requests to other Golem agent endpoints, the request body must match the **Golem HTTP body mapping convention**: non-binary body parameters are always deserialized from a **JSON object** where each top-level field corresponds to a method parameter name. This is true even when the endpoint has a single body parameter.

For example, given an agent endpoint:

```moonbit
#derive.endpoint(post = "/record")
pub fn RecorderAgent::record(self : Self, body : String) -> Unit { ... }
```

The correct HTTP request body is:

```moonbit
// ✅ CORRECT — JSON object with parameter name as key
let json_body = "{\"body\": \"hello\"}"

// ❌ WRONG — raw string does NOT match Golem's JSON body mapping
let json_body = "\"hello\""
```

> **Rule of thumb:** If the target endpoint is a Golem agent, always send `Content-Type: application/json` with parameter names as JSON keys.

## Resource Lifecycle

WASI HTTP uses resource handles that must be dropped in the correct order:

1. **OutputStream** must be dropped before calling `OutgoingBody::finish`
2. **OutgoingBody** must be finished (not just dropped) to signal the body is complete
3. **InputStream** must be dropped before calling `IncomingBody::finish`
4. **IncomingBody** must be finished to signal you're done reading

Dropping a resource out of order will cause a trap.

## Key Constraints

- All HTTP types are WASI resources with strict ownership and drop ordering
- Field values (`field-value`) are `FixedArray[Byte]`, not strings — use byte literals (`b"..."`) or `.to_utf8_bytes()`
- The `blocking_read` function reads up to the requested number of bytes — for large responses, read in a loop
- HTTP requests are automatically durably persisted by Golem — no manual durability wrapping is needed
- The `Method` variant defaults to `Get` when constructing an `OutgoingRequest`
