---
name: golem-add-cors-moonbit
description: "Configuring CORS allowed origins for HTTP endpoints in MoonBit Golem agents. Use when the user asks to enable CORS, allow cross-origin requests, or configure allowed origins."
---

# Configuring CORS for MoonBit HTTP Endpoints

## Mount-Level CORS

Set `#derive.mount_cors(...)` on the agent struct to apply allowed origins to **all** endpoints:

```moonbit
#derive.agent
#derive.mount("/api/{name}")
#derive.mount_cors("https://app.example.com")
pub(all) struct MyAgent {
  name : String
}

fn MyAgent::new(name : String) -> MyAgent {
  { name }
}

#derive.endpoint(get="/data")
pub fn MyAgent::get_data(self : Self) -> String {
  // Allows https://app.example.com
  "data"
}
```

Multiple origins can be specified as separate arguments:

```moonbit
#derive.mount_cors("https://app.example.com", "https://other.example.com")
```

## Endpoint-Level CORS

Set `#derive.endpoint_cors(...)` on a method to add allowed origins for a specific endpoint. Origins are **unioned** with mount-level CORS:

```moonbit
#derive.agent
#derive.mount("/api/{name}")
#derive.mount_cors("https://app.example.com")
pub(all) struct MyAgent {
  name : String
}

fn MyAgent::new(name : String) -> MyAgent {
  { name }
}

#derive.endpoint(get="/data")
#derive.endpoint_cors("*")
pub fn MyAgent::get_data(self : Self) -> String {
  // Allows BOTH https://app.example.com AND * (all origins)
  "data"
}

#derive.endpoint(get="/other")
pub fn MyAgent::get_other(self : Self) -> String {
  // Inherits mount-level: only https://app.example.com
  "other"
}
```

## Wildcard

Use `"*"` to allow all origins:

```moonbit
#derive.agent
#derive.mount("/public/{name}")
#derive.mount_cors("*")
pub(all) struct PublicAgent {
  name : String
}
```

## CORS Preflight

Golem automatically handles `OPTIONS` preflight requests for endpoints that have CORS configured. The preflight response includes `Access-Control-Allow-Origin`, `Access-Control-Allow-Methods`, and `Access-Control-Allow-Headers` headers.
