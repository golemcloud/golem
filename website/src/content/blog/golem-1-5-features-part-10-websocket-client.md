---
title: "Golem 1.5 features — Part 10: WebSocket client"
date: "2026-04-18T11:50:30Z"
author: "Daniel Vigovszky"
tags: ["Product Updates", "Engineering Articles"]
slug: "golem-1-5-features-part-10-websocket-client"
originalUrl: "https://blog.vigoo.dev/posts/golem15-part10-websocket/"
---

## Introduction

This post is part of a series showcasing Golem 1.5 features. Golem applications are WebAssembly components and the only way they can make external requests is through the WASI HTTP interface. Check the [other Golem-related posts](https://blog.vigoo.dev/tags/golem/) for more information.

## WebSockets

The WASI HTTP interface has limitations, particularly lacking support for upgrading to WebSocket connections. Golem 1.5 addresses this with a new WebSocket client API.

### WebSocket client API

The API is defined through a WebAssembly interface (`golem:websocket@1.5.0`) that includes:

- Error variants for connection, send, receive, protocol, and closure failures
- A `message` variant supporting text or binary data
- A `websocket-connection` resource with methods for:
  - `connect()`: Establish connections to ws:// or wss:// servers
  - `send()`: Transmit messages
  - `receive()`: Get next message (blocking)
  - `receive-with-timeout()`: Get message with timeout
  - `close()`: Send close frame
  - `subscribe()`: Return pollable for message availability

### Higher level WebSocket APIs

Language-specific implementations vary:

- **TypeScript**: Uses standard browser `WebSocket` and `WebSocketStream` APIs
- **Rust**: The SDK provides its own implementation inspired by tungstenite
- **Scala**: Compiles to JS, utilizing browser APIs
- **MoonBit**: Accesses low-level WIT bindings directly

### Examples

```typescript
@agent()
class ExampleAgent extends BaseAgent {
  async run(): Promise<void> {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket("wss://example.com/chat");

      ws.onopen = () => {
        console.log("Connected");
        ws.send("Hello, server!");
      };

      ws.onmessage = (event: MessageEvent) => {
        if (typeof event.data === "string") {
          console.log("Text:", event.data);
        } else {
          console.log("Binary:", new Uint8Array(event.data));
        }
      };

      ws.onerror = () => reject(new Error("WebSocket error"));
      ws.onclose = (event: CloseEvent) => {
        console.log(`Closed [${event.code}] "${event.reason}"`);
        resolve();
      };
    });
  }
}
```

```rust
#[agent_implementation]
impl ExampleAgent for ExampleAgentImpl {
    async fn run() -> Result<(), WebSocketError> {
        let ws = WebsocketConnection::connect("wss://example.com/chat", None)?;
        println!("Connected");

        ws.send(&WebSocketMessage::Text("Hello, server!".to_string()))?;

        loop {
            match ws.receive().await {
                Ok(WebSocketMessage::Text(text)) => println!("Text: {text}"),
                Ok(WebSocketMessage::Binary(data)) => println!("Binary: {data:?}"),
                Err(WebSocketError::Closed(info)) => {
                    if let Some(info) = info {
                        println!("Closed [{}] \"{}\"", info.code, info.reason);
                    }
                    break;
                }
                Err(err) => return Err(err),
            }
        }

        Ok(())
    }
}
```

```scala
case class ExampleAgentImpl() extends ExampleAgent {
  def run(): Future[Unit] = {
    val done = Promise[Unit]()
    val ws = new WebSocket("wss://example.com/chat")

    ws.onopen = { (_: Event) =>
      println("Connected")
      ws.send("Hello, server!")
    }

    ws.onmessage = { (event: MessageEvent) =>
      event.data match {
        case text: String => println(s"Text: $text")
        case other        => println(s"Binary: $other")
      }
    }

    ws.onerror = { (_: Event) =>
      done.tryFailure(new Exception("WebSocket error"))
    }

    ws.onclose = { (event: CloseEvent) =>
      println(s"Closed [${event.code}] \"${event.reason}\"")
      done.trySuccess(())
    }

    done.future
  }
}
```

```moonbit
pub fn ExampleAgent::run(self : Self) -> Unit raise @common.AgentError {
  let conn = match @websocket_client.WebsocketConnection::connect(
    "wss://example.com/chat", None,
  ) {
    Ok(c) => c
    Err(e) => raise @common.AgentError::InvalidInput("Connect failed: \{e}")
  }
  println("Connected")
  match conn.send(Text("Hello, server!")) {
    Ok(_) => ()
    Err(e) => raise @common.AgentError::InvalidInput("Send failed: \{e}")
  }
  while true {
    match conn.receive() {
      Ok(Text(msg)) => println("Text: \{msg}")
      Ok(Binary(data)) => println("Binary: \{data.length()} bytes")
      Err(Closed(Some(info))) => {
        println("Closed [\{info.code}] \"\{info.reason}\"")
        break
      }
      Err(Closed(None)) => break
      Err(e) => raise @common.AgentError::InvalidInput("Receive failed: \{e}")
    }
  }
  conn.drop()
}
```

### Durability

Golem agents survive failures through durable state, but WebSocket recovery presents challenges. The system supports transparent reconnection if servers support it, though this remains a development area for future releases.
