///|
/// Drops every still-owned capability reachable from a recursive schema value.
/// Repeated references and distinct holders for the same raw resource are
/// canonicalized so malformed values still issue at most one resource drop.
#doc(hidden)
pub fn drop_owned_capabilities(value : SchemaValue) -> Unit {
  let secrets : Array[@types.Secret] = []
  let quota_tokens : Array[@types.QuotaToken] = []
  let permission_cards : Array[@types.PermissionCard] = []
  take_owned_capabilities(value, secrets, quota_tokens, permission_cards)
  for secret in secrets {
    secret.drop()
  }
  for quota_token in quota_tokens {
    quota_token.drop()
  }
  for permission_card in permission_cards {
    permission_card.drop()
  }
}

///|
fn[T : Eq] push_unique(values : Array[T], value : T) -> Unit {
  for existing in values {
    if existing == value {
      return
    }
  }
  values.push(value)
}

///|
fn take_owned_capabilities(
  value : SchemaValue,
  secrets : Array[@types.Secret],
  quota_tokens : Array[@types.QuotaToken],
  permission_cards : Array[@types.PermissionCard],
) -> Unit {
  match value {
    Secret(handle) =>
      match handle.take() {
        Some(raw) => push_unique(secrets, raw)
        None => ()
      }
    QuotaToken(handle) =>
      match handle.take() {
        Some(raw) => push_unique(quota_tokens, raw)
        None => ()
      }
    PermissionCard(handle) =>
      match handle.take() {
        Some(raw) => push_unique(permission_cards, raw)
        None => ()
      }
    Record(values) | Tuple(values) | List(values) | FixedList(values) =>
      for nested in values {
        take_owned_capabilities(nested, secrets, quota_tokens, permission_cards)
      }
    Variant(_, nested)
    | Option(nested)
    | ResultOk(nested)
    | ResultErr(nested) =>
      match nested {
        Some(nested) =>
          take_owned_capabilities(
            nested, secrets, quota_tokens, permission_cards,
          )
        None => ()
      }
    Map(entries) =>
      for entry in entries {
        take_owned_capabilities(
          entry.key,
          secrets,
          quota_tokens,
          permission_cards,
        )
        take_owned_capabilities(
          entry.value,
          secrets,
          quota_tokens,
          permission_cards,
        )
      }
    Union(_, nested) =>
      take_owned_capabilities(nested, secrets, quota_tokens, permission_cards)
    _ => ()
  }
}

///|
/// Drops each distinct capability handle stored in a flat wire value exactly
/// once. This is used when malformed wire values cannot be lifted into the
/// recursive SDK model and no decoded owner exists to release them.
#doc(hidden)
pub fn drop_wit_capabilities(value : @types.SchemaValueTree) -> Unit {
  let secrets : Array[@types.Secret] = []
  let quota_tokens : Array[@types.QuotaToken] = []
  let permission_cards : Array[@types.PermissionCard] = []
  for node in value.value_nodes {
    match node {
      @types.SecretValue(raw) => push_unique(secrets, raw)
      @types.QuotaTokenHandle(raw) => push_unique(quota_tokens, raw)
      @types.PermissionCardHandle(raw) => push_unique(permission_cards, raw)
      _ => ()
    }
  }
  for secret in secrets {
    secret.drop()
  }
  for quota_token in quota_tokens {
    quota_token.drop()
  }
  for permission_card in permission_cards {
    permission_card.drop()
  }
}
