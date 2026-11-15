///|
/// Error type for reading the principal of a 1.6 snapshot envelope.
pub suberror JsonCodecError {
  JsonCodecError(String)
} derive(Debug)

///|
pub extend JsonCodecError with Debug::{to_repr}

///|
fn uuid_to_json_string(uuid : @types.Uuid) -> String {
  @schema.Uuid::from_wit(uuid).to_string()
}

///|
fn uuid_from_json_string(s : String) -> @types.Uuid raise JsonCodecError {
  @schema.Uuid::parse(s).to_wit() catch {
    @schema.UuidParseError(message) => raise JsonCodecError(message)
  }
}

///|
/// Checks the principal of a 1.6 snapshot envelope, and discards it.
///
/// A 2.0 agent reads such a snapshot as strictly as before (a malformed principal still fails
/// the load) and then drops the principal: the caller of an invocation comes from the host,
/// never from saved state.
pub fn validate_legacy_principal_json(json : Json) -> Unit raise JsonCodecError {
  let obj = match json {
    Object(m) => m
    _ => raise JsonCodecError("Expected JSON object for Principal")
  }
  let tag = match obj.get("tag") {
    Some(String(s)) => s
    _ =>
      raise JsonCodecError("Missing or invalid 'tag' field in Principal JSON")
  }
  match tag {
    "anonymous" => ()
    "oidc" => {
      let val = match obj.get("val") {
        Some(Object(m)) => m
        _ => raise JsonCodecError("Missing 'val' for oidc principal")
      }
      for key in ["sub", "issuer", "claims"] {
        match val.get(key) {
          Some(String(_)) => ()
          _ => raise JsonCodecError("Missing '\{key}' in oidc principal")
        }
      }
    }
    "agent" => {
      let val = match obj.get("val") {
        Some(Object(m)) => m
        _ => raise JsonCodecError("Missing 'val' for agent principal")
      }
      match val.get("componentId") {
        Some(String(s)) => ignore(uuid_from_json_string(s))
        _ => raise JsonCodecError("Missing 'componentId' in agent val")
      }
      match val.get("agentId") {
        Some(String(_)) => ()
        _ => raise JsonCodecError("Missing 'agentId' in agent val")
      }
    }
    "golem-user" => {
      let val = match obj.get("val") {
        Some(Object(m)) => m
        _ => raise JsonCodecError("Missing 'val' for golem-user principal")
      }
      match val.get("accountId") {
        Some(String(s)) => ignore(uuid_from_json_string(s))
        _ => raise JsonCodecError("Missing 'accountId' in golem-user principal")
      }
    }
    other => raise JsonCodecError("Unknown principal tag: \{other}")
  }
}
