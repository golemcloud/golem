// Who caused the running invocation.
//
// An agent method, a constructor, a tool or a tool middleware that wants its caller declares an
// `InvocationContext` parameter and reads `InvocationContext::caller`. The generator injects the
// context: it is not part of the input schema and callers never supply it.
//
// A `Caller` and a `Principal` are live values, not data. They have no schema and cannot be a
// method input or result, nor a field of one. To store who called, keep the `PrincipalRef` of
// `ctx.caller().original`: it is plain data.

///|
/// The identity of one principal: an issuer and a subject.
///
/// A reference is data: holding one grants nothing. Two references name the same principal
/// exactly when both strings are equal, byte for byte. Its text form is
/// `("<issuer>","<subject>")`, and `PrincipalRef::parse` refuses every other spelling. As method
/// data it is the named record `principal-ref { issuer, subject }`.
pub(all) struct PrincipalRef {
  issuer : String
  subject : String
} derive(Eq, Hash, Debug)

///|
pub fn PrincipalRef::new(issuer : String, subject : String) -> PrincipalRef {
  { issuer, subject, }
}

///|
/// Byte equality of both strings.
pub fn PrincipalRef::equals(self : PrincipalRef, other : PrincipalRef) -> Bool {
  self.issuer == other.issuer && self.subject == other.subject
}

///|
pub suberror PrincipalRefParseError {
  PrincipalRefParseError(String)
} derive(Debug)

///|
fn hex4(code : Int32) -> String {
  let digits = "0123456789abcdef"
  let mut out = ""
  for shift in [12, 8, 4, 0] {
    out = out +
      Int32::unsafe_to_char(digits[(code >> shift) & 15].to_int()).to_string()
  }
  out
}

///|
fn write_ref_string(value : String) -> String {
  let buf = StringBuilder()
  buf.write_char('"')
  for c in value {
    match c {
      '"' => buf.write_string("\\\"")
      '\\' => buf.write_string("\\\\")
      '\u{08}' => buf.write_string("\\b")
      '\t' => buf.write_string("\\t")
      '\n' => buf.write_string("\\n")
      '\u{0c}' => buf.write_string("\\f")
      '\r' => buf.write_string("\\r")
      _ =>
        if c.to_int() < 0x20 {
          buf.write_string("\\u" + hex4(c.to_int()))
        } else {
          buf.write_char(c)
        }
    }
  }
  buf.write_char('"')
  buf.to_string()
}

///|
/// The canonical text form `("<issuer>","<subject>")`.
pub fn PrincipalRef::to_string(self : PrincipalRef) -> String {
  "(" +
  write_ref_string(self.issuer) +
  "," +
  write_ref_string(self.subject) +
  ")"
}

///|
pub impl Show for PrincipalRef with fn output(self, logger) {
  logger.write_string(PrincipalRef::to_string(self))
}

///|
fn ref_fail(
  what : String,
  position : Int32,
) -> Unit raise PrincipalRefParseError {
  raise PrincipalRefParseError(
    "invalid principal reference text: \{what} at character \{position}",
  )
}

///|
fn ref_expect(
  chars : Array[Char],
  position : Ref[Int32],
  expected : Char,
) -> Unit raise PrincipalRefParseError {
  if position.val < chars.length() && chars[position.val] == expected {
    position.val += 1
  } else {
    ref_fail("expected `\{expected}`", position.val)
  }
}

///|
fn ref_string(
  chars : Array[Char],
  position : Ref[Int32],
) -> String raise PrincipalRefParseError {
  ref_expect(chars, position, '"')
  let buf = StringBuilder()
  while true {
    if position.val >= chars.length() {
      ref_fail("unterminated string", position.val)
    }
    let c = chars[position.val]
    position.val += 1
    if c == '"' {
      break
    } else if c == '\\' {
      buf.write_char(ref_escape(chars, position))
    } else if c.to_int() < 0x20 {
      ref_fail("unescaped control character", position.val - 1)
    } else {
      buf.write_char(c)
    }
  }
  buf.to_string()
}

///|
/// Reads the escape after a backslash. Only the escapes that the printer writes are accepted,
/// so every character has exactly one spelling.
fn ref_escape(
  chars : Array[Char],
  position : Ref[Int32],
) -> Char raise PrincipalRefParseError {
  if position.val >= chars.length() {
    ref_fail("non-canonical escape", position.val)
  }
  let c = chars[position.val]
  position.val += 1
  match c {
    '"' => '"'
    '\\' => '\\'
    'b' => '\u{08}'
    't' => '\t'
    'n' => '\n'
    'f' => '\u{0c}'
    'r' => '\r'
    'u' => {
      if position.val + 4 > chars.length() {
        ref_fail("non-canonical escape", position.val)
      }
      let mut code = 0
      for i in 0..<4 {
        let h = chars[position.val + i]
        let d = if h >= '0' && h <= '9' {
          h.to_int() - '0'.to_int()
        } else if h >= 'a' && h <= 'f' {
          h.to_int() - 'a'.to_int() + 10
        } else {
          ref_fail("non-canonical escape", position.val)
          0
        }
        code = code * 16 + d
      }
      if code >= 0x20 ||
        code == 0x08 ||
        code == 0x09 ||
        code == 0x0a ||
        code == 0x0c ||
        code == 0x0d {
        ref_fail("non-canonical escape", position.val)
      }
      position.val += 4
      Int32::unsafe_to_char(code)
    }
    _ => {
      ref_fail("non-canonical escape", position.val - 1)
      '?'
    }
  }
}

///|
/// Reads the canonical text form `("<issuer>","<subject>")`. Every other spelling is refused.
pub fn PrincipalRef::parse(
  text : String,
) -> PrincipalRef raise PrincipalRefParseError {
  let chars : Array[Char] = []
  for c in text {
    chars.push(c)
  }
  let position = Ref(0)
  ref_expect(chars, position, '(')
  let issuer = ref_string(chars, position)
  ref_expect(chars, position, ',')
  let subject = ref_string(chars, position)
  ref_expect(chars, position, ')')
  if position.val < chars.length() {
    ref_fail("trailing text", position.val)
  }
  { issuer, subject, }
}

///|
fn ref_from_wit(r : @common.PrincipalRef) -> PrincipalRef {
  { issuer: r.issuer, subject: r.subject, }
}

///|
const PRINCIPAL_REF_TYPE_ID : String = "principal-ref"

///|
pub impl @schema.IntoSchema for PrincipalRef with fn type_id() {
  PRINCIPAL_REF_TYPE_ID
}

///|
pub impl @schema.IntoSchema for PrincipalRef with fn register_in(b) {
  b.register(
    PRINCIPAL_REF_TYPE_ID,
    () => {
      @model.schema_type(
        Record([
          @model.field(
            "issuer",
            @schema.register_in_with(
              b,
              (@schema.TypeTag::{ } : @schema.TypeTag[String]),
            ),
          ),
          @model.field(
            "subject",
            @schema.register_in_with(
              b,
              (@schema.TypeTag::{ } : @schema.TypeTag[String]),
            ),
          ),
        ]),
      )
    },
    Some("principal-ref"),
  )
}

///|
pub impl @schema.IntoSchema for PrincipalRef with fn to_value(self) {
  Record([@schema.to_value_as(self.issuer), @schema.to_value_as(self.subject)])
}

///|
pub impl @schema.FromSchema for PrincipalRef with fn from_value(v) {
  let fields = @schema.expect_record(v, "PrincipalRef")
  let issuer : String = @schema.from_value_as(
    @schema.record_field(fields, 0, "PrincipalRef"),
  )
  let subject : String = @schema.from_value_as(
    @schema.record_field(fields, 1, "PrincipalRef"),
  )
  { issuer, subject, }
}

///|
/// The kind of a `Principal`.
pub(all) enum PrincipalKind {
  User
  Account
  Agent
  Tool
  Anonymous
} derive(Eq, Debug)

///|
pub fn PrincipalKind::to_string(self : PrincipalKind) -> String {
  match self {
    User => "user"
    Account => "account"
    Agent => "agent"
    Tool => "tool"
    Anonymous => "anonymous"
  }
}

///|
/// A person who signed in through an identity provider. `claims` is the exact JSON object text
/// that the host recorded.
pub(all) struct UserPrincipal {
  principal_ref : PrincipalRef
  claims : String
  authenticated_at : @types.Datetime
}

///|
/// How an account principal proved itself.
pub(all) enum AccountAuthentication {
  ApiToken
  BootstrapToken
  SignIn
} derive(Eq, Debug)

///|
/// A Golem account, as seen through the CLI, the REST API or a token.
pub(all) struct AccountPrincipal {
  principal_ref : PrincipalRef
  account_id : @types.AccountId
  authentication : AccountAuthentication
}

///|
/// A running agent that made a call.
pub(all) struct AgentPrincipal {
  principal_ref : PrincipalRef
  agent_id : @types.AgentId
  agent_type : String
  environment_id : @types.EnvironmentId
}

///|
/// A tool that made a call.
pub(all) struct ToolPrincipal {
  principal_ref : PrincipalRef
  environment_id : @types.EnvironmentId
  tool_name : String
}

///|
/// One side of a call: a user, an account, an agent, a tool or nobody.
pub(all) enum Principal {
  User(UserPrincipal)
  Account(AccountPrincipal)
  Agent(AgentPrincipal)
  Tool(ToolPrincipal)
  Anonymous
}

///|
pub fn Principal::kind(self : Principal) -> PrincipalKind {
  match self {
    User(_) => PrincipalKind::User
    Account(_) => PrincipalKind::Account
    Agent(_) => PrincipalKind::Agent
    Tool(_) => PrincipalKind::Tool
    Anonymous => PrincipalKind::Anonymous
  }
}

///|
/// The identity of this principal, or `None` for an anonymous one.
pub fn Principal::principal_ref(self : Principal) -> PrincipalRef? {
  match self {
    User(p) => Some(p.principal_ref)
    Account(p) => Some(p.principal_ref)
    Agent(p) => Some(p.principal_ref)
    Tool(p) => Some(p.principal_ref)
    Anonymous => None
  }
}

///|
/// Whether this principal is the one `other` names. An anonymous principal equals nothing.
pub fn Principal::equals(self : Principal, other : Principal) -> Bool {
  match (self.principal_ref(), other.principal_ref()) {
    (Some(a), Some(b)) => a.equals(b)
    _ => false
  }
}

///|
/// Whether this principal is the one `other` names. An anonymous principal equals nothing.
pub fn Principal::equals_ref(self : Principal, other : PrincipalRef) -> Bool {
  match self.principal_ref() {
    Some(a) => a.equals(other)
    None => false
  }
}

///|
/// `"<kind> <ref text>"`, or `"anonymous"`. It never includes claims.
pub fn Principal::to_string(self : Principal) -> String {
  match self.principal_ref() {
    Some(r) =>
      PrincipalKind::to_string(self.kind()) + " " + PrincipalRef::to_string(r)
    None => "anonymous"
  }
}

///|
pub impl Show for Principal with fn output(self, logger) {
  logger.write_string(Principal::to_string(self))
}

///|
pub fn Principal::from_wit(p : @common.Principal) -> Principal {
  match p {
    User(u) =>
      User({
        principal_ref: ref_from_wit(u.pair),
        claims: u.claims,
        authenticated_at: u.authenticated_at,
      })
    Account(a) =>
      Account({
        principal_ref: ref_from_wit(a.pair),
        account_id: a.account_id,
        authentication: match a.authentication {
          API_TOKEN => ApiToken
          BOOTSTRAP_TOKEN => BootstrapToken
          SIGN_IN => SignIn
        },
      })
    Agent(a) =>
      Agent({
        principal_ref: ref_from_wit(a.pair),
        agent_id: a.agent_id,
        agent_type: a.agent_type,
        environment_id: a.environment_id,
      })
    Tool(t) =>
      Tool({
        principal_ref: ref_from_wit(t.pair),
        environment_id: t.environment_id,
        tool_name: t.tool_name,
      })
    Anonymous => Anonymous
  }
}

///|
/// Which token a caller presented and who made it. The id is the token's own, never its secret.
pub(all) struct TokenProvenance {
  id : @types.Uuid
  maker : PrincipalRef?
}

///|
/// Who caused the running invocation: the `original` principal that started the chain, the
/// `immediate` principal that made this call, and the provenance of the tokens on the way.
pub(all) struct Caller {
  original : Principal
  immediate : Principal
  tokens : Array[TokenProvenance]
}

///|
/// A caller with no identity: anonymous in both roles and no tokens.
pub fn Caller::anonymous() -> Caller {
  { original: Anonymous, immediate: Anonymous, tokens: [], }
}

///|
pub fn Caller::from_wit(c : @common.Caller) -> Caller {
  {
    original: Principal::from_wit(c.original),
    immediate: Principal::from_wit(c.immediate),
    tokens: c.tokens.map(t => { id: t.id, maker: t.maker.map(ref_from_wit), }),
  }
}

///|
/// What the host tells an invocation about itself. The host is asked for the caller on first
/// use, and the answer is kept for the rest of the invocation.
pub struct InvocationContext {
  priv mut cached : Caller?
  priv read : () -> Caller
}

///|
/// The context of the running invocation, read from the host on first use.
pub fn InvocationContext::current() -> InvocationContext {
  { cached: None, read: () => Caller::from_wit(@invocationContext.caller()), }
}

///|
/// The context of an invocation made by `caller`.
pub fn InvocationContext::from_caller(caller : Caller) -> InvocationContext {
  { cached: Some(caller), read: () => caller, }
}

///|
/// The context of an invocation that nobody is behind.
pub fn InvocationContext::anonymous() -> InvocationContext {
  InvocationContext::from_caller(Caller::anonymous())
}

///|
/// Who caused the running invocation.
pub fn InvocationContext::caller(self : InvocationContext) -> Caller {
  match self.cached {
    Some(c) => c
    None => {
      let c = (self.read)()
      self.cached = Some(c)
      c
    }
  }
}
