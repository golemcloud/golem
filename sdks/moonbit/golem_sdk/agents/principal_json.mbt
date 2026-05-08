///|
/// Error type for JSON codec operations on Principal and related types.
pub suberror JsonCodecError {
  JsonCodecError(String)
} derive(Show)

///|
let hex_chars : FixedArray[Char] = [
  '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
]

///|
fn uint64_hex(value : UInt64, width : Int) -> String {
  let buf = StringBuilder::new()
  for i = width - 1; i >= 0; i = i - 1 {
    let nibble = ((value >> (i * 4)) & 0xFUL).to_int()
    buf.write_char(hex_chars[nibble])
  }
  buf.to_string()
}

///|
fn hex_code_to_uint64(c : Int) -> UInt64 raise JsonCodecError {
  if c >= 0x30 && c <= 0x39 {
    (c - 0x30).to_uint64()
  } else if c >= 0x61 && c <= 0x66 {
    (c - 0x61 + 10).to_uint64()
  } else if c >= 0x41 && c <= 0x46 {
    (c - 0x41 + 10).to_uint64()
  } else {
    raise JsonCodecError("Invalid hex character code: \{c}")
  }
}

///|
pub fn uuid_to_standard_string(uuid : @rpcTypes.Uuid) -> String {
  let hi = uuid.high_bits
  let lo = uuid.low_bits
  let p1 = uint64_hex((hi >> 32) & 0xFFFFFFFFUL, 8)
  let p2 = uint64_hex((hi >> 16) & 0xFFFFUL, 4)
  let p3 = uint64_hex(hi & 0xFFFFUL, 4)
  let p4 = uint64_hex((lo >> 48) & 0xFFFFUL, 4)
  let p5 = uint64_hex(lo & 0xFFFFFFFFFFFFUL, 12)
  "\{p1}-\{p2}-\{p3}-\{p4}-\{p5}"
}

///|
pub fn uuid_from_standard_string(
  s : String,
) -> @rpcTypes.Uuid raise JsonCodecError {
  let mut hi : UInt64 = 0UL
  let mut lo : UInt64 = 0UL
  let mut hex_count = 0
  for i = 0; i < s.length(); i = i + 1 {
    let c = s[i].to_int()
    if c == 0x2D {
      continue
    }
    let d = hex_code_to_uint64(c)
    if hex_count < 16 {
      hi = (hi << 4) | d
    } else {
      lo = (lo << 4) | d
    }
    hex_count = hex_count + 1
  }
  if hex_count != 32 {
    raise JsonCodecError(
      "Invalid UUID string: expected 32 hex digits, got \{hex_count}",
    )
  }
  { high_bits: hi, low_bits: lo }
}

///|
/// Serializes a `Principal` to JSON, matching the format used by Rust/TS SDKs.
pub fn principal_to_json(principal : @common.Principal) -> Json {
  match principal {
    Anonymous => Json::object({ "tag": Json::string("anonymous") })
    Oidc(oidc) => {
      let fields : Map[String, Json] = {}
      fields["sub"] = Json::string(oidc.sub)
      fields["issuer"] = Json::string(oidc.issuer)
      fields["email"] = match oidc.email {
        Some(v) => Json::string(v)
        None => Json::null()
      }
      fields["name"] = match oidc.name {
        Some(v) => Json::string(v)
        None => Json::null()
      }
      fields["emailVerified"] = match oidc.email_verified {
        Some(v) => Json::boolean(v)
        None => Json::null()
      }
      fields["givenName"] = match oidc.given_name {
        Some(v) => Json::string(v)
        None => Json::null()
      }
      fields["familyName"] = match oidc.family_name {
        Some(v) => Json::string(v)
        None => Json::null()
      }
      fields["picture"] = match oidc.picture {
        Some(v) => Json::string(v)
        None => Json::null()
      }
      fields["preferredUsername"] = match oidc.preferred_username {
        Some(v) => Json::string(v)
        None => Json::null()
      }
      fields["claims"] = Json::string(oidc.claims)
      Json::object({ "tag": Json::string("oidc"), "val": Json::object(fields) })
    }
    Agent(agent) =>
      Json::object({
        "tag": Json::string("agent"),
        "val": agent_id_to_json(agent.agent_id),
      })
    GolemUser(user) =>
      Json::object({
        "tag": Json::string("golem-user"),
        "val": Json::object({ "accountId": account_id_to_json(user.account_id) }),
      })
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
      let email_verified : Bool? = match val.get("emailVerified") {
        Some(True) => Some(true)
        Some(False) => Some(false)
        _ => None
      }
      let given_name : String? = match val.get("givenName") {
        Some(String(s)) => Some(s)
        _ => None
      }
      let family_name : String? = match val.get("familyName") {
        Some(String(s)) => Some(s)
        _ => None
      }
      let picture : String? = match val.get("picture") {
        Some(String(s)) => Some(s)
        _ => None
      }
      let preferred_username : String? = match val.get("preferredUsername") {
        Some(String(s)) => Some(s)
        _ => None
      }
      let claims = match val.get("claims") {
        Some(String(s)) => s
        _ => raise JsonCodecError("Missing 'claims' in oidc principal")
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
        Some(Object(_)) as json => json.unwrap()
        _ => raise JsonCodecError("Missing 'val' for agent principal")
      }
      let agent_id = agent_id_from_json(val)
      @common.Principal::Agent({ agent_id, })
    }
    "golem-user" => {
      let val = match obj.get("val") {
        Some(Object(m)) => m
        _ => raise JsonCodecError("Missing 'val' for golem-user principal")
      }
      let account_id_str = match val.get("accountId") {
        Some(String(s)) => s
        _ => raise JsonCodecError("Missing 'accountId' in golem-user principal")
      }
      @common.Principal::GolemUser({
        account_id: { uuid: uuid_from_standard_string(account_id_str) },
      })
    }
    other => raise JsonCodecError("Unknown principal tag: \{other}")
  }
}

///|
fn agent_id_to_json(id : @rpcTypes.AgentId) -> Json {
  Json::object({
    "componentId": Json::string(uuid_to_standard_string(id.component_id.uuid)),
    "agentId": Json::string(id.agent_id),
  })
}

///|
fn agent_id_from_json(json : Json) -> @rpcTypes.AgentId raise JsonCodecError {
  let obj = match json {
    Object(m) => m
    _ => raise JsonCodecError("Expected JSON object for AgentId")
  }
  let component_id_str = match obj.get("componentId") {
    Some(String(s)) => s
    _ => raise JsonCodecError("Missing 'componentId' in agent val")
  }
  let agent_id = match obj.get("agentId") {
    Some(String(s)) => s
    _ => raise JsonCodecError("Missing 'agentId' in agent val")
  }
  {
    component_id: { uuid: uuid_from_standard_string(component_id_str) },
    agent_id,
  }
}

///|
fn account_id_to_json(id : @rpcTypes.AccountId) -> Json {
  Json::string(uuid_to_standard_string(id.uuid))
}
