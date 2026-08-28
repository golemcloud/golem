///|
/// Drops every still-owned capability reachable from a recursive schema value.
/// Repeated references and distinct holders for the same raw resource are
/// canonicalized so malformed values still issue at most one resource drop.
#doc(hidden)
pub fn drop_owned_capabilities(value : SchemaValue) -> Unit {
  let secrets : Array[@types.Secret] = []
  let quota_tokens : Array[@types.QuotaToken] = []
  let permission_cards : Array[@types.PermissionCard] = []
  let wrapped_streams : Array[@types.SchemaValueStream] = []
  let native_streams : Array[@async-core.Stream[@types.SchemaValueTree]] = []
  take_owned_capabilities(
    value, secrets, quota_tokens, permission_cards, wrapped_streams, native_streams,
  )
  for secret in secrets {
    secret.drop()
  }
  for quota_token in quota_tokens {
    quota_token.drop()
  }
  for permission_card in permission_cards {
    permission_card.drop()
  }
  for stream in wrapped_streams {
    stream.drop()
  }
  for stream in native_streams {
    stream.drop_sync()
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
fn push_unique_native_stream(
  values : Array[@async-core.Stream[@types.SchemaValueTree]],
  value : @async-core.Stream[@types.SchemaValueTree],
) -> Unit {
  for existing in values {
    if physical_equal(existing, value) {
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
  wrapped_streams : Array[@types.SchemaValueStream],
  native_streams : Array[@async-core.Stream[@types.SchemaValueTree]],
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
    Stream(handle) =>
      match handle.take() {
        Some(Wrapped(raw)) => push_unique(wrapped_streams, raw)
        Some(Native(raw)) => push_unique_native_stream(native_streams, raw)
        None => ()
      }
    Record(values) | Tuple(values) | List(values) | FixedList(values) =>
      for nested in values {
        take_owned_capabilities(
          nested, secrets, quota_tokens, permission_cards, wrapped_streams, native_streams,
        )
      }
    Variant(_, nested)
    | Option(nested)
    | ResultOk(nested)
    | ResultErr(nested) =>
      match nested {
        Some(nested) =>
          take_owned_capabilities(
            nested, secrets, quota_tokens, permission_cards, wrapped_streams, native_streams,
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
          wrapped_streams,
          native_streams,
        )
        take_owned_capabilities(
          entry.value,
          secrets,
          quota_tokens,
          permission_cards,
          wrapped_streams,
          native_streams,
        )
      }
    Union(_, nested) =>
      take_owned_capabilities(
        nested, secrets, quota_tokens, permission_cards, wrapped_streams, native_streams,
      )
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
  let streams : Array[@types.SchemaValueStream] = []
  for node in value.value_nodes {
    match node {
      @types.SecretValue(raw) => push_unique(secrets, raw)
      @types.QuotaTokenHandle(raw) => push_unique(quota_tokens, raw)
      @types.PermissionCardHandle(raw) => push_unique(permission_cards, raw)
      @types.StreamValue(raw) => push_unique(streams, raw)
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
  for stream in streams {
    stream.drop()
  }
}
