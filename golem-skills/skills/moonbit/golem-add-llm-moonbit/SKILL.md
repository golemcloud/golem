---
name: golem-add-llm-moonbit
description: "Adding LLM and AI capabilities to a MoonBit Golem agent. Use when the user wants to add LLM chat, embeddings, or any AI provider integration to a MoonBit agent."
---

# Adding LLM and AI Capabilities (MoonBit)

## Overview

There are no AI-specific libraries for MoonBit. To integrate with LLM providers, call the provider's REST API directly using **WASI HTTP** — the same HTTP mechanism available for all outgoing requests in MoonBit Golem agents.

Load the `golem-make-http-request-moonbit` skill for full details on making HTTP requests from MoonBit agents.

## Calling an LLM API

Here is how to call the OpenAI Chat Completions API using WASI HTTP:

```moonbit
fn chat_completion(prompt : String, api_key : String) -> String {
  let body = "{\"model\": \"gpt-4o\", \"messages\": [{\"role\": \"user\", \"content\": \""
    + prompt
    + "\"}]}"

  // Create headers
  let headers = @http.Fields::from_list(
    [
      ("Content-Type", b"application/json"),
      ("Authorization", ("Bearer " + api_key).to_utf8_bytes()),
    ],
  ).unwrap()

  // Create POST request
  let request = @http.OutgoingRequest::new(headers)
  let _ = request.set_method(@http.Post)
  let _ = request.set_scheme(Some(@http.Https))
  let _ = request.set_authority(Some("api.openai.com"))
  let _ = request.set_path_with_query(Some("/v1/chat/completions"))

  // Write request body
  let out_body = request.body().unwrap()
  let output_stream = out_body.write().unwrap()
  output_stream.blocking_write_and_flush(body.to_utf8_bytes()).unwrap()
  output_stream.drop()
  @http.OutgoingBody::finish(out_body, None).unwrap()

  // Send and wait for response
  let future_response = @http.handle(request, None).unwrap()
  let pollable = future_response.subscribe()
  pollable.block()
  let response = future_response.get().unwrap().unwrap().unwrap()

  // Read response body
  let incoming_body = response.consume().unwrap()
  let stream = incoming_body.stream().unwrap()
  let bytes = stream.blocking_read(1048576UL).unwrap()
  stream.drop()
  @http.IncomingBody::finish(incoming_body)

  // Parse the response JSON to extract the message content
  let response_text = String::from_utf8_lossy(bytes)
  // Use your JSON parsing approach to extract choices[0].message.content
  response_text
}
```

## Setting API Keys

Store provider API keys as **secrets** using Golem's typed config system. Load the `golem-add-secret-moonbit` skill for full details. In brief, declare the key in a config struct:

```moonbit
#derive.config
pub(all) struct MyAgentConfig {
  api_key : @config.Secret[String]
}
```

Then manage it via the CLI:

```shell
golem agent-secret create api_key --secret-type String --secret-value "sk-..."
```

Access in code with `self.config.value.api_key.get!()`.

## Calling Other Providers

The same WASI HTTP approach works for any LLM provider — change the authority, path, headers, and request body to match the provider's API:

| Provider | Authority | Path | Auth Header |
|----------|-----------|------|-------------|
| OpenAI | `api.openai.com` | `/v1/chat/completions` | `Bearer $OPENAI_API_KEY` |
| Anthropic | `api.anthropic.com` | `/v1/messages` | `x-api-key: $ANTHROPIC_API_KEY` |
| Google Gemini | `generativelanguage.googleapis.com` | `/v1beta/models/{model}:generateContent?key=$API_KEY` | API key in URL |
| Groq | `api.groq.com` | `/openai/v1/chat/completions` | `Bearer $GROQ_API_KEY` |
| Mistral | `api.mistral.ai` | `/v1/chat/completions` | `Bearer $MISTRAL_API_KEY` |

## Complete Agent Example

```moonbit
#derive.agent
pub(all) struct ChatAgent {
  chat_name : String
  mut messages : String
}

///|
fn ChatAgent::new(chat_name : String) -> ChatAgent {
  let system_msg = "{\"role\": \"system\", \"content\": \"You are a helpful assistant for chat '"
    + chat_name
    + "'\"}"
  { chat_name, messages: system_msg }
}

///|
#derive.endpoint(post = "/ask")
pub fn ChatAgent::ask(self : Self, question : String) -> String {
  // Build messages array
  let user_msg = "{\"role\": \"user\", \"content\": \"" + question + "\"}"
  let all_messages = "[" + self.messages + ", " + user_msg + "]"

  let body = "{\"model\": \"gpt-4o\", \"messages\": " + all_messages + "}"

  let api_key = @env.var("OPENAI_API_KEY").unwrap()

  let headers = @http.Fields::from_list(
    [
      ("Content-Type", b"application/json"),
      ("Authorization", ("Bearer " + api_key).to_utf8_bytes()),
    ],
  ).unwrap()

  let request = @http.OutgoingRequest::new(headers)
  let _ = request.set_method(@http.Post)
  let _ = request.set_scheme(Some(@http.Https))
  let _ = request.set_authority(Some("api.openai.com"))
  let _ = request.set_path_with_query(Some("/v1/chat/completions"))

  let out_body = request.body().unwrap()
  let output_stream = out_body.write().unwrap()
  output_stream.blocking_write_and_flush(body.to_utf8_bytes()).unwrap()
  output_stream.drop()
  @http.OutgoingBody::finish(out_body, None).unwrap()

  let future_response = @http.handle(request, None).unwrap()
  let pollable = future_response.subscribe()
  pollable.block()
  let response = future_response.get().unwrap().unwrap().unwrap()

  let incoming_body = response.consume().unwrap()
  let stream = incoming_body.stream().unwrap()
  let bytes = stream.blocking_read(1048576UL).unwrap()
  stream.drop()
  @http.IncomingBody::finish(incoming_body)

  let reply = String::from_utf8_lossy(bytes)
  // Update messages for next turn
  let assistant_msg = "{\"role\": \"assistant\", \"content\": \"...\"}"
  self.messages = self.messages + ", " + user_msg + ", " + assistant_msg
  reply
}
```

## Key Constraints

- There are no AI-specific libraries for MoonBit — call provider REST APIs directly using WASI HTTP
- Load the `golem-make-http-request-moonbit` skill for full HTTP request patterns, error handling, and resource lifecycle rules
- API keys should be stored as secrets using Golem's typed config system (load the `golem-add-secret-moonbit` skill)
- All HTTP requests are automatically durably persisted by Golem — responses are replayed from the oplog on recovery
- Field values in WASI HTTP headers are `FixedArray[Byte]` — use byte literals (`b"..."`) or `.to_utf8_bytes()`
