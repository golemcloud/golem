///|
struct MatrixSecret {
  handle : @model.GuestSecretHandle
}

///|
fn MatrixSecret::load(key : Array[String]) -> MatrixSecret {
  let inner = @schema.wire_schema(
    (@schema.type_tag() : @schema.TypeTag[String]),
  )
  let nodes = inner.type_nodes.copy()
  let root = nodes.length()
  nodes.push({
    body: @types.SecretType({ inner: inner.root, category: None, }),
    metadata: @schema.empty_wire_metadata(),
  })
  let tree = match
    @agentHost.get_config_value(key, {
      type_nodes: nodes,
      defs: inner.defs,
      root,
    }) {
    Ok(value) => value
    Err(_) => abort("failed to load matrix secret")
  }
  let handle : @model.GuestSecretHandle = @schema.from_wire(tree) catch {
    _ => abort("matrix secret config returned an invalid handle")
  }
  { handle, }
}

///|
fn MatrixSecret::reveals_expected(self : MatrixSecret) -> Bool {
  let raw = match self.handle.with_handle(raw => raw) {
    Some(raw) => raw
    None => abort("matrix secret was already transferred")
  }
  let revealed = match
    @secretReveal.reveal(
      raw,
      @schema.wire_schema((@schema.type_tag() : @schema.TypeTag[String])),
    ) {
    Ok(value) => value
    Err(_) => abort("matrix secret reveal failed")
  }
  let value : String = @schema.from_wire(revealed) catch {
    _ => abort("matrix secret reveal returned an invalid value")
  }
  value == "matrix-secret-value"
}

///|
fn MatrixSecret::drop(self : MatrixSecret) -> Unit {
  match self.handle.take() {
    Some(raw) => raw.drop()
    None => ()
  }
}

///|
impl @schema.IntoWire for MatrixSecret with fn append_wire(self, encoder) {
  @schema.IntoWire::append_wire(self.handle, encoder)
}

///|
impl @schema.IntoWire for MatrixSecret with fn release_wire(self, releaser) {
  @schema.IntoWire::release_wire(self.handle, releaser)
}

///|
impl @schema.FromWire for MatrixSecret with fn read_wire(reader, index) {
  { handle: @schema.FromWire::read_wire(reader, index), }
}

///|
impl @schema.IntoSchema for MatrixSecret with fn type_id() {
  "component.MatrixSecret"
}

///|
impl @schema.IntoSchema for MatrixSecret with fn register_in(builder) {
  @model.schema_type(
    Secret({
      inner: @schema.register_in_with(
        builder,
        (@schema.type_tag() : @schema.TypeTag[String]),
      ),
      category: None,
    }),
  )
}

///|
impl @schema.IntoSchema for MatrixSecret with fn to_value(self) {
  Secret(self.handle)
}

///|
impl @schema.FromSchema for MatrixSecret with fn from_value(value) {
  match value {
    Secret(handle) => { handle, }
    other =>
      raise @schema.FromSchemaError::shape_mismatch(
        "secret",
        @schema.value_kind(other),
        "MatrixSecret",
      )
  }
}

///|
impl @schema.WireSchema for MatrixSecret with fn append_wire_type(encoder) {
  let inner = @schema.append_wire_type_with(
    encoder,
    (@schema.type_tag() : @schema.TypeTag[String]),
  )
  encoder.push(@types.SecretType({ inner, category: None, }))
}

///|
struct MatrixQuotaToken {
  handle : @model.GuestQuotaTokenHandle
}

///|
fn MatrixQuotaToken::new() -> MatrixQuotaToken {
  {
    handle: @model.GuestQuotaTokenHandle::new(
      @quotaRaw.new_token("matrix-capacity", 1UL),
    ),
  }
}

///|
fn MatrixQuotaToken::reserve_and_commit_one(self : MatrixQuotaToken) -> Bool {
  let result = match
    self.handle.with_handle(raw => @quotaRaw.reserve(raw, 1UL)) {
    Some(result) => result
    None => abort("matrix quota token was already transferred")
  }
  match result {
    Ok(reservation) => {
      reservation.commit(1UL)
      true
    }
    Err(_) => false
  }
}

///|
fn MatrixQuotaToken::is_present(self : MatrixQuotaToken) -> Bool {
  self.handle.is_present()
}

///|
fn MatrixQuotaToken::drop(self : MatrixQuotaToken) -> Unit {
  match self.handle.take() {
    Some(raw) => raw.drop()
    None => ()
  }
}

///|
impl @schema.IntoWire for MatrixQuotaToken with fn append_wire(self, encoder) {
  @schema.IntoWire::append_wire(self.handle, encoder)
}

///|
impl @schema.IntoWire for MatrixQuotaToken with fn release_wire(self, releaser) {
  @schema.IntoWire::release_wire(self.handle, releaser)
}

///|
impl @schema.FromWire for MatrixQuotaToken with fn read_wire(reader, index) {
  { handle: @schema.FromWire::read_wire(reader, index), }
}

///|
impl @schema.IntoSchema for MatrixQuotaToken with fn type_id() {
  "component.MatrixQuotaToken"
}

///|
impl @schema.IntoSchema for MatrixQuotaToken with fn register_in(_builder) {
  @model.schema_type(QuotaToken({ resource_name: Some("matrix-capacity"), }))
}

///|
impl @schema.IntoSchema for MatrixQuotaToken with fn to_value(self) {
  QuotaToken(self.handle)
}

///|
impl @schema.FromSchema for MatrixQuotaToken with fn from_value(value) {
  match value {
    QuotaToken(handle) => { handle, }
    other =>
      raise @schema.FromSchemaError::shape_mismatch(
        "quota-token",
        @schema.value_kind(other),
        "MatrixQuotaToken",
      )
  }
}

///|
impl @schema.WireSchema for MatrixQuotaToken with fn append_wire_type(encoder) {
  encoder.push(
    @types.QuotaTokenType({ resource_name: Some("matrix-capacity"), }),
  )
}

///|
struct MatrixPermissionCard {
  handle : @model.GuestPermissionCardHandle
}

///|
fn MatrixPermissionCard::drop(self : MatrixPermissionCard) -> Unit {
  match self.handle.take() {
    Some(raw) => raw.drop()
    None => ()
  }
}

///|
impl @schema.IntoWire for MatrixPermissionCard with fn append_wire(
  self,
  encoder,
) {
  @schema.IntoWire::append_wire(self.handle, encoder)
}

///|
impl @schema.IntoWire for MatrixPermissionCard with fn release_wire(
  self,
  releaser,
) {
  @schema.IntoWire::release_wire(self.handle, releaser)
}

///|
impl @schema.FromWire for MatrixPermissionCard with fn read_wire(reader, index) {
  { handle: @schema.FromWire::read_wire(reader, index), }
}

///|
impl @schema.IntoSchema for MatrixPermissionCard with fn type_id() {
  "component.MatrixPermissionCard"
}

///|
impl @schema.IntoSchema for MatrixPermissionCard with fn register_in(_builder) {
  @model.schema_type(PermissionCard({ polymorphic: false, }))
}

///|
impl @schema.IntoSchema for MatrixPermissionCard with fn to_value(self) {
  PermissionCard(self.handle)
}

///|
impl @schema.FromSchema for MatrixPermissionCard with fn from_value(value) {
  match value {
    PermissionCard(handle) => { handle, }
    other =>
      raise @schema.FromSchemaError::shape_mismatch(
        "permission-card",
        @schema.value_kind(other),
        "MatrixPermissionCard",
      )
  }
}

///|
impl @schema.WireSchema for MatrixPermissionCard with fn append_wire_type(
  encoder,
) {
  encoder.push(@types.PermissionCardType({ polymorphic: false, }))
}

///|
#derive.golem_schema
pub(all) struct SecretExchange {
  provider : String
  principal : String
  ownerAgentId : String
  revealed : Bool
  secret : MatrixSecret
}

///|
#derive.golem_schema
pub(all) struct QuotaExchange {
  provider : String
  principal : String
  ownerAgentId : String
  reserved : Bool
  token : MatrixQuotaToken
}

///|
#derive.golem_schema
pub(all) struct PermissionExchange {
  provider : String
  principal : String
  ownerAgentId : String
  card : MatrixPermissionCard
}

///|
#derive.tool("matrix-resource", version="1.0.0")
struct MatrixResource {}

///|
#derive.command(subtree="MatrixResourceSecret")
pub fn MatrixResource::secret() -> Unit {

}

///|
#derive.tool("secret")
struct MatrixResourceSecret {}

///|
#derive.command(subtree="MatrixResourceQuota")
pub fn MatrixResource::quota() -> Unit {

}

///|
#derive.tool("quota")
struct MatrixResourceQuota {}

///|
#derive.command(subtree="MatrixResourcePermissions")
pub fn MatrixResource::permissions() -> Unit {

}

///|
#derive.tool("permissions")
struct MatrixResourcePermissions {}

///|
#derive.command(subtree="MatrixResourceTyped")
pub fn MatrixResource::typed() -> Unit {

}

///|
#derive.tool("typed")
struct MatrixResourceTyped {}

///|
fn matrix_resource_principal(principal : @tool.Principal) -> String {
  match principal {
    Anonymous => "anonymous"
    Oidc(oidc) => "oidc:" + oidc.sub
    Agent(_) => "agent"
    GolemUser(_) => "golem-user"
  }
}

///|
pub fn MatrixResourceSecret::exchange(
  secret : MatrixSecret,
  principal : @tool.Principal,
) -> SecretExchange {
  {
    provider: "moonbit",
    principal: matrix_resource_principal(principal),
    ownerAgentId: @api.get_self_metadata().agent_id.agent_id,
    revealed: secret.reveals_expected(),
    secret,
  }
}

///|
pub fn MatrixResourceQuota::exchange(
  token : MatrixQuotaToken,
  principal : @tool.Principal,
) -> QuotaExchange {
  {
    provider: "moonbit",
    principal: matrix_resource_principal(principal),
    ownerAgentId: @api.get_self_metadata().agent_id.agent_id,
    reserved: token.reserve_and_commit_one(),
    token,
  }
}

///|
pub fn MatrixResourcePermissions::exchange(
  card : MatrixPermissionCard,
  principal : @tool.Principal,
) -> PermissionExchange {
  {
    provider: "moonbit",
    principal: matrix_resource_principal(principal),
    ownerAgentId: @api.get_self_metadata().agent_id.agent_id,
    card,
  }
}

///|
pub fn MatrixResourceTyped::transform(
  input : @schema.AgentStream[UInt],
) -> @schema.AgentStream[UInt] {
  @schema.AgentStream::produce(
    async fn(writer) {
      defer input.drop()
      while input.read() is Some(value) {
        if writer.write_one(value * 3U + 1U) is PeerDropped {
          return
        }
      }
    },
    on_unstarted_drop=() => input.drop(),
  )
}

///|
#derive.config
struct MatrixResourceConfig {
  secret : @config.Secret[String]
}

///|
#derive.agent
struct MoonBitResourceToolStreamingCaller {
  name : String
  secret : @config.Secret[String]
}

///|
fn MoonBitResourceToolStreamingCaller::new(
  name : String,
  config : @config.Config[MatrixResourceConfig],
) -> MoonBitResourceToolStreamingCaller {
  { name, secret: config.value.secret, }
}

///|
#derive.golem_schema
struct MatrixResourceObservation {
  secretFirstProvider : String
  secretSecondProvider : String
  secretFirstRevealed : Bool
  secretSecondRevealed : Bool
  secretPrincipal : String
  secretOwnerAgentId : String
  quotaProvider : String
  quotaReserved : Bool
  quotaReturnedUsable : Bool
  quotaOriginalConsumed : Bool
  quotaPrincipal : String
  quotaOwnerAgentId : String
  permissionSupported : Bool
  permissionProvider : String
  permissionSameIdentity : Bool
  permissionOriginalConsumed : Bool
  permissionPrincipal : String
  permissionOwnerAgentId : String
  typedValues : Array[UInt]
}

///|
pub async fn MoonBitResourceToolStreamingCaller::matrix_resource_observation(
  self : Self,
) -> MatrixResourceObservation {
  let client = MatrixResourceClient::new()
  defer client.drop()

  let secret_client = client.secret()
  defer secret_client.drop()
  let first = match
    secret_client.exchange(MatrixSecret::load(self.secret.key)) {
    Ok(value) => value
    Err(_) => abort("first matrix secret exchange failed")
  }
  let second = match secret_client.exchange(first.secret) {
    Ok(value) => value
    Err(_) => abort("second matrix secret exchange failed")
  }
  defer second.secret.drop()

  let quota_client = client.quota()
  defer quota_client.drop()
  let original_token = MatrixQuotaToken::new()
  let quota = match quota_client.exchange(original_token) {
    Ok(value) => value
    Err(_) => abort("matrix quota exchange failed")
  }
  let quota_original_consumed = !original_token.is_present()
  let quota_returned_usable = quota.token.reserve_and_commit_one()
  defer quota.token.drop()

  let typed_client = client.typed()
  defer typed_client.drop()
  let input : @schema.AgentStream[UInt] = @schema.AgentStream::produce(async fn(
    writer,
  ) {
    guard writer.write_all([2U, 5U, 9U]) is Accepted else {
      abort("matrix typed input was dropped")
    }
  })
  let output = match typed_client.transform(input) {
    Ok(value) => value
    Err(_) => abort("matrix typed transform failed")
  }
  defer output.drop()
  let typed_values : Array[UInt] = []
  while output.read() is Some(value) {
    typed_values.push(value)
  }

  {
    secretFirstProvider: first.provider,
    secretSecondProvider: second.provider,
    secretFirstRevealed: first.revealed,
    secretSecondRevealed: second.revealed,
    secretPrincipal: second.principal,
    secretOwnerAgentId: second.ownerAgentId,
    quotaProvider: quota.provider,
    quotaReserved: quota.reserved,
    quotaReturnedUsable: quota_returned_usable,
    quotaOriginalConsumed: quota_original_consumed,
    quotaPrincipal: quota.principal,
    quotaOwnerAgentId: quota.ownerAgentId,
    permissionSupported: false,
    permissionProvider: "",
    permissionSameIdentity: false,
    permissionOriginalConsumed: false,
    permissionPrincipal: "",
    permissionOwnerAgentId: "",
    typedValues: typed_values,
  }
}
