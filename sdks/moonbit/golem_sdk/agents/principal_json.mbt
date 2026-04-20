///|
/// Error type for JSON codec operations on Principal and related types.
pub suberror JsonCodecError {
  JsonCodecError(String)
} derive(Show)

///|
/// Serializes a `Principal` to JSON, matching the format used by Rust/TS SDKs.
pub fn principal_to_json(principal : @common.Principal) -> Json {
  match principal {
    Anonymous => Json::object({ "tag": Json::string("anonymous") })
    Oidc(oidc) => {
      let fields : Map[String, Json] = {}
      fields["sub"] = Json::string(oidc.sub)
      fields["issuer"] = Json::string(oidc.issuer)
      match oidc.email {
        Some(v) => fields["email"] = Json::string(v)
        None => ()
      }
      match oidc.name {
        Some(v) => fields["name"] = Json::string(v)
        None => ()
      }
      match oidc.email_verified {
        Some(v) => fields["email_verified"] = Json::boolean(v)
        None => ()
      }
      match oidc.given_name {
        Some(v) => fields["given_name"] = Json::string(v)
        None => ()
      }
      match oidc.family_name {
        Some(v) => fields["family_name"] = Json::string(v)
        None => ()
      }
      match oidc.picture {
        Some(v) => fields["picture"] = Json::string(v)
        None => ()
      }
      match oidc.preferred_username {
        Some(v) => fields["preferred_username"] = Json::string(v)
        None => ()
      }
      fields["claims"] = Json::string(oidc.claims)
      Json::object({ "tag": Json::string("oidc"), "val": Json::object(fields) })
    }
    Agent(agent) => {
      let agent_id_json = agent_id_to_json(agent.agent_id)
      Json::object({
        "tag": Json::string("agent"),
        "val": Json::object({ "agent_id": agent_id_json }),
      })
    }
    GolemUser(user) => {
      let account_id_json = account_id_to_json(user.account_id)
      Json::object({
        "tag": Json::string("golem-user"),
        "val": Json::object({ "account_id": account_id_json }),
      })
    }
  }
}

///|
/// Deserializes a `Principal` from JSON.
pub fn principal_from_json(
  json : Json,
) -> @common.Principal raise JsonCodecError {
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
    "anonymous" => @common.Principal::Anonymous
    "oidc" => {
      let val = match obj.get("val") {
        Some(Object(m)) => m
        _ => raise JsonCodecError("Missing 'val' for oidc principal")
      }
      let sub = match val.get("sub") {
        Some(String(s)) => s
        _ => raise JsonCodecError("Missing 'sub' in oidc principal")
      }
      let issuer = match val.get("issuer") {
        Some(String(s)) => s
        _ => raise JsonCodecError("Missing 'issuer' in oidc principal")
      }
      let email : String? = match val.get("email") {
        Some(String(s)) => Some(s)
        _ => None
      }
      let name : String? = match val.get("name") {
        Some(String(s)) => Some(s)
        _ => None
      }
      let email_verified : Bool? = match val.get("email_verified") {
        Some(True) => Some(true)
        Some(False) => Some(false)
        _ => None
      }
      let given_name : String? = match val.get("given_name") {
        Some(String(s)) => Some(s)
        _ => None
      }
      let family_name : String? = match val.get("family_name") {
        Some(String(s)) => Some(s)
        _ => None
      }
      let picture : String? = match val.get("picture") {
        Some(String(s)) => Some(s)
        _ => None
      }
      let preferred_username : String? = match val.get("preferred_username") {
        Some(String(s)) => Some(s)
        _ => None
      }
      let claims = match val.get("claims") {
        Some(String(s)) => s
        _ => ""
      }
      @common.Principal::Oidc({
        sub,
        issuer,
        email,
        name,
        email_verified,
        given_name,
        family_name,
        picture,
        preferred_username,
        claims,
      })
    }
    "agent" => {
      let val = match obj.get("val") {
        Some(Object(m)) => m
        _ => raise JsonCodecError("Missing 'val' for agent principal")
      }
      let agent_id = match val.get("agent_id") {
        Some(json) => agent_id_from_json(json)
        _ => raise JsonCodecError("Missing 'agent_id' in agent principal")
      }
      @common.Principal::Agent({ agent_id, })
    }
    "golem-user" => {
      let val = match obj.get("val") {
        Some(Object(m)) => m
        _ => raise JsonCodecError("Missing 'val' for golem-user principal")
      }
      let account_id = match val.get("account_id") {
        Some(json) => account_id_from_json(json)
        _ =>
          raise JsonCodecError("Missing 'account_id' in golem-user principal")
      }
      @common.Principal::GolemUser({ account_id, })
    }
    other => raise JsonCodecError("Unknown principal tag: \{other}")
  }
}

///|
fn uuid_to_json(uuid : @rpcTypes.Uuid) -> Json {
  Json::object({
    "high_bits": Json::number(uuid.high_bits.to_double()),
    "low_bits": Json::number(uuid.low_bits.to_double()),
  })
}

///|
fn uuid_from_json(json : Json) -> @rpcTypes.Uuid raise JsonCodecError {
  let obj = match json {
    Object(m) => m
    _ => raise JsonCodecError("Expected JSON object for Uuid")
  }
  let high_bits = match obj.get("high_bits") {
    Some(Number(n, ..)) => n.to_uint64()
    _ => raise JsonCodecError("Missing 'high_bits' in Uuid")
  }
  let low_bits = match obj.get("low_bits") {
    Some(Number(n, ..)) => n.to_uint64()
    _ => raise JsonCodecError("Missing 'low_bits' in Uuid")
  }
  { high_bits, low_bits }
}

///|
fn component_id_to_json(id : @rpcTypes.ComponentId) -> Json {
  Json::object({ "uuid": uuid_to_json(id.uuid) })
}

///|
fn component_id_from_json(
  json : Json,
) -> @rpcTypes.ComponentId raise JsonCodecError {
  let obj = match json {
    Object(m) => m
    _ => raise JsonCodecError("Expected JSON object for ComponentId")
  }
  let uuid = match obj.get("uuid") {
    Some(json) => uuid_from_json(json)
    _ => raise JsonCodecError("Missing 'uuid' in ComponentId")
  }
  { uuid, }
}

///|
fn agent_id_to_json(id : @rpcTypes.AgentId) -> Json {
  Json::object({
    "component_id": component_id_to_json(id.component_id),
    "agent_id": Json::string(id.agent_id),
  })
}

///|
fn agent_id_from_json(json : Json) -> @rpcTypes.AgentId raise JsonCodecError {
  let obj = match json {
    Object(m) => m
    _ => raise JsonCodecError("Expected JSON object for AgentId")
  }
  let component_id = match obj.get("component_id") {
    Some(json) => component_id_from_json(json)
    _ => raise JsonCodecError("Missing 'component_id' in AgentId")
  }
  let agent_id = match obj.get("agent_id") {
    Some(String(s)) => s
    _ => raise JsonCodecError("Missing 'agent_id' in AgentId")
  }
  { component_id, agent_id }
}

///|
fn account_id_to_json(id : @rpcTypes.AccountId) -> Json {
  Json::object({ "uuid": uuid_to_json(id.uuid) })
}

///|
fn account_id_from_json(
  json : Json,
) -> @rpcTypes.AccountId raise JsonCodecError {
  let obj = match json {
    Object(m) => m
    _ => raise JsonCodecError("Expected JSON object for AccountId")
  }
  let uuid = match obj.get("uuid") {
    Some(json) => uuid_from_json(json)
    _ => raise JsonCodecError("Missing 'uuid' in AccountId")
  }
  { uuid, }
}
