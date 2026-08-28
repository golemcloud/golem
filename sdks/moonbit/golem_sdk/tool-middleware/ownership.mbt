///|
/// Capability classes that can occur in a schema value carried across a tool
/// middleware boundary.
pub(all) enum ResourceKind {
  Secret
  QuotaToken
  PermissionCard
  SchemaValueStream
} derive(Eq, Debug)

///|
/// One candidate resource observed while scanning a newly acquired wire
/// carrier. The constructor is hidden from generated documentation because it
/// is guest-boundary plumbing, not an authoring API.
pub struct ResourceCandidate {
  priv kind : ResourceKind
  priv raw_index : Int
  priv drop_raw : () -> Unit
}

///|
#doc(hidden)
pub fn resource_candidate(
  kind : ResourceKind,
  raw_index : Int,
  drop_raw : () -> Unit,
) -> ResourceCandidate {
  { kind, raw_index, drop_raw }
}

///|
/// Exact identity of one locally acquired owned-resource lifetime. A raw
/// canonical-ABI table index can be reused, so the local generation is part of
/// the identity even though the token's fields remain private.
pub struct ResourceAcquisitionToken {
  priv kind : ResourceKind
  priv raw_index : Int
  priv generation : Int
} derive(Eq, Debug)

///|
priv enum ResourceOwnershipState {
  ResourceAvailable
  ResourceTransferredLocal
  ResourceTransferredUnderlying
  ResourceTransferredFinal
  ResourceDropped
} derive(Eq)

///|
priv struct ResourceAcquisition {
  token : ResourceAcquisitionToken
  mut state : ResourceOwnershipState
  drop_raw : () -> Unit
}

///|
priv struct InvocationOwnershipState {
  mut active : Bool
  mut revoked : Bool
  mut raw_dropped : Bool
  mut next_generation : Int
  drop_raw : () -> Unit
  acquisitions : Array[ResourceAcquisition]
  cleanups : Array[() -> Unit]
}

///|
/// Shared ownership state for one middleware guest invocation. Every copied
/// underlying wrapper and every owned carrier created for the invocation holds
/// this same context.
pub struct OwnershipContext {
  priv state : Ref[InvocationOwnershipState]
}

///|
#doc(hidden)
pub fn OwnershipContext::new(drop_raw : () -> Unit) -> OwnershipContext {
  {
    state: {
      val: {
        active: false,
        revoked: false,
        raw_dropped: false,
        next_generation: 0,
        drop_raw,
        acquisitions: [],
        cleanups: [],
      },
    },
  }
}

///|
fn OwnershipContext::register_cleanup(
  self : OwnershipContext,
  cleanup : () -> Unit,
) -> Unit {
  self.state.val.cleanups.push(cleanup)
  if self.state.val.revoked {
    cleanup()
  }
}

///|
fn[E] OwnershipContext::begin_call(
  self : OwnershipContext,
) -> Result[Unit, ToolInvokeError[E]] {
  if self.state.val.revoked {
    return Err(InvalidInput("underlying tool is no longer available"))
  }
  if self.state.val.active {
    return Err(InvalidInput("underlying tool invocation is already active"))
  }
  self.state.val.active = true
  Ok(())
}

///|
fn OwnershipContext::finish_call(self : OwnershipContext) -> Unit {
  self.state.val.active = false
  self.drop_raw_if_idle()
}

///|
fn OwnershipContext::drop_raw_if_idle(self : OwnershipContext) -> Unit {
  if self.state.val.revoked &&
    !self.state.val.active &&
    !self.state.val.raw_dropped {
    self.state.val.raw_dropped = true
    (self.state.val.drop_raw)()
  }
}

///|
fn OwnershipContext::is_revoked(self : OwnershipContext) -> Bool {
  self.state.val.revoked
}

///|
fn OwnershipContext::find_exact_acquisition(
  self : OwnershipContext,
  token : ResourceAcquisitionToken,
) -> ResourceAcquisition? {
  for acquisition in self.state.val.acquisitions {
    if acquisition.token == token {
      return Some(acquisition)
    }
  }
  None
}

///|
fn OwnershipContext::find_available_acquisition(
  self : OwnershipContext,
  kind : ResourceKind,
  raw_index : Int,
) -> ResourceAcquisition? {
  for acquisition in self.state.val.acquisitions {
    if acquisition.token.kind == kind &&
      acquisition.token.raw_index == raw_index &&
      acquisition.state == ResourceAvailable {
      return Some(acquisition)
    }
  }
  None
}

///|
fn contains_resource_key(
  tokens : Array[ResourceAcquisitionToken],
  kind : ResourceKind,
  raw_index : Int,
) -> Bool {
  for token in tokens {
    if token.kind == kind && token.raw_index == raw_index {
      return true
    }
  }
  false
}

///|
fn ResourceKind::label(self : ResourceKind) -> String {
  match self {
    Secret => "secret"
    QuotaToken => "quota-token"
    PermissionCard => "permission-card"
    SchemaValueStream => "schema-value-stream"
  }
}

///|
/// Opaque ownership attachment for one unchanged raw WIT carrier. Copies share
/// ledger state through the context and therefore cannot transfer or drop the
/// same acquisitions twice.
pub struct ResourceCarrierOwnership {
  priv context : OwnershipContext
  priv tokens : Array[ResourceAcquisitionToken]
  priv valid : Bool
}

///|
/// Mints exact acquisition tokens for all unique resources in one newly lifted
/// carrier. Duplicate nodes in that carrier keep one owner but invalidate the
/// carrier. A resource already available through an older carrier also
/// invalidates the new carrier without claiming or dropping the older owner.
#doc(hidden)
pub fn OwnershipContext::acquire_resources(
  self : OwnershipContext,
  candidates : Array[ResourceCandidate],
) -> ResourceCarrierOwnership {
  let tokens : Array[ResourceAcquisitionToken] = []
  let mut valid = !self.state.val.revoked
  for candidate in candidates {
    if contains_resource_key(tokens, candidate.kind, candidate.raw_index) {
      valid = false
      continue
    }
    if self.find_available_acquisition(candidate.kind, candidate.raw_index)
      is Some(_) {
      valid = false
      continue
    }
    let token = {
      kind: candidate.kind,
      raw_index: candidate.raw_index,
      generation: self.state.val.next_generation,
    }
    self.state.val.next_generation = self.state.val.next_generation + 1
    self.state.val.acquisitions.push({
      token,
      state: ResourceAvailable,
      drop_raw: candidate.drop_raw,
    })
    tokens.push(token)
  }
  let carrier = { context: self, tokens, valid }
  if self.state.val.revoked {
    ignore(carrier.drop())
  }
  carrier
}

///|
fn ResourceCarrierOwnership::transition(
  self : ResourceCarrierOwnership,
  target : ResourceOwnershipState,
) -> Result[Unit, String] {
  if !self.valid && target != ResourceDropped {
    return Err(
      "resource carrier is invalid because it contains duplicate ownership",
    )
  }
  if self.context.is_revoked() && target != ResourceDropped {
    return Err("middleware invocation ownership has been revoked")
  }
  let acquisitions : Array[ResourceAcquisition] = []
  for token in self.tokens {
    let acquisition = match self.context.find_exact_acquisition(token) {
      Some(acquisition) => acquisition
      None => return Err("resource acquisition token is not recognized")
    }
    if acquisition.state != ResourceAvailable {
      return Err(
        "resource acquisition is no longer available: \{token.kind.label()}:\{token.raw_index}:\{token.generation}",
      )
    }
    acquisitions.push(acquisition)
  }
  for acquisition in acquisitions {
    acquisition.state = target
  }
  if target == ResourceDropped {
    for acquisition in acquisitions {
      (acquisition.drop_raw)()
    }
  }
  Ok(())
}

///|
#doc(hidden)
fn ResourceCarrierOwnership::check_available(
  self : ResourceCarrierOwnership,
) -> Result[Unit, String] {
  if !self.valid {
    return Err(
      "resource carrier is invalid because it contains duplicate ownership",
    )
  }
  if self.context.is_revoked() {
    return Err("middleware invocation ownership has been revoked")
  }
  for token in self.tokens {
    let acquisition = match self.context.find_exact_acquisition(token) {
      Some(acquisition) => acquisition
      None => return Err("resource acquisition token is not recognized")
    }
    if acquisition.state != ResourceAvailable {
      return Err(
        "resource acquisition is no longer available: \{token.kind.label()}:\{token.raw_index}:\{token.generation}",
      )
    }
  }
  Ok(())
}

///|
#doc(hidden)
fn ResourceCarrierOwnership::transfer_local(
  self : ResourceCarrierOwnership,
) -> Result[Unit, String] {
  self.transition(ResourceTransferredLocal)
}

///|
#doc(hidden)
pub fn ResourceCarrierOwnership::transfer_underlying(
  self : ResourceCarrierOwnership,
) -> Result[Unit, String] {
  self.transition(ResourceTransferredUnderlying)
}

///|
#doc(hidden)
pub fn ResourceCarrierOwnership::transfer_final(
  self : ResourceCarrierOwnership,
) -> Result[Unit, String] {
  self.transition(ResourceTransferredFinal)
}

///|
#doc(hidden)
pub fn ResourceCarrierOwnership::drop(
  self : ResourceCarrierOwnership,
) -> Result[Unit, String] {
  self.transition(ResourceDropped)
}

///|
priv enum StdinOwnershipState {
  StdinAvailable
  StdinTransferredUnderlying
  StdinDropped
} derive(Eq)

///|
priv struct MiddlewareStdinState {
  mut stream : @async-core.Stream[Byte]?
  mut ownership : StdinOwnershipState
}

///|
/// Opaque one-shot owner of the outer middleware stdin stream.
pub struct MiddlewareStdin {
  priv context : OwnershipContext
  priv state : Ref[MiddlewareStdinState]
}

///|
#doc(hidden)
pub fn OwnershipContext::own_stdin(
  self : OwnershipContext,
  stream : @async-core.Stream[Byte],
) -> MiddlewareStdin {
  let state : Ref[MiddlewareStdinState] = {
    val: { stream: Some(stream), ownership: StdinAvailable },
  }
  let owned : MiddlewareStdin = { context: self, state }
  self.register_cleanup(fn() { owned.drop_if_available() })
  owned
}

///|
fn MiddlewareStdin::drop_if_available(self : MiddlewareStdin) -> Unit {
  if self.state.val.ownership == StdinAvailable {
    let stream = self.state.val.stream
    self.state.val.stream = None
    self.state.val.ownership = StdinDropped
    match stream {
      Some(stream) => stream.drop_sync()
      None => ()
    }
  }
}

///|
fn MiddlewareStdin::check_for_underlying(
  self : MiddlewareStdin,
  context : OwnershipContext,
) -> Result[Unit, String] {
  if !physical_equal(self.context.state, context.state) {
    return Err("middleware stdin belongs to another invocation")
  }
  if self.context.is_revoked() {
    return Err("middleware invocation ownership has been revoked")
  }
  if self.state.val.ownership != StdinAvailable || self.state.val.stream is None {
    return Err("middleware stdin is no longer available")
  }
  Ok(())
}

///|
#doc(hidden)
pub fn MiddlewareStdin::take_for_underlying(
  self : MiddlewareStdin,
) -> Result[@async-core.Stream[Byte], String] {
  if self.context.is_revoked() {
    return Err("middleware invocation ownership has been revoked")
  }
  if self.state.val.ownership != StdinAvailable {
    return Err("middleware stdin is no longer available")
  }
  let stream = match self.state.val.stream {
    Some(stream) => stream
    None => return Err("middleware stdin is no longer available")
  }
  self.state.val.stream = None
  self.state.val.ownership = StdinTransferredUnderlying
  Ok(stream)
}

///|
#doc(hidden)
pub fn MiddlewareStdin::drop(self : MiddlewareStdin) -> Result[Unit, String] {
  if self.state.val.ownership != StdinAvailable {
    return Err("middleware stdin is no longer available")
  }
  self.drop_if_available()
  Ok(())
}

///|
priv enum InvocationResultOwnershipState {
  ResultAvailable
  ResultProjected
  ResultTransferredFinal
  ResultDropped
} derive(Eq)

///|
priv struct OwnedInvocationResultState {
  mut result : @toolCore.InvocationResult?
  mut ownership : InvocationResultOwnershipState
  cleanup : (@toolCore.InvocationResult) -> Unit
}

///|
/// Opaque owner of a decoded underlying result. The value and stdout remain a
/// single cleanup unit until projection, final transfer, or abandonment.
pub struct OwnedInvocationResult {
  priv context : OwnershipContext
  priv state : Ref[OwnedInvocationResultState]
}

///|
fn OwnershipContext::own_result(
  self : OwnershipContext,
  result : @toolCore.InvocationResult,
) -> OwnedInvocationResult {
  self.own_result_with_cleanup(result, @toolCore.drop_invocation_result)
}

///|
fn OwnershipContext::own_result_with_cleanup(
  self : OwnershipContext,
  result : @toolCore.InvocationResult,
  cleanup : (@toolCore.InvocationResult) -> Unit,
) -> OwnedInvocationResult {
  let state : Ref[OwnedInvocationResultState] = {
    val: { result: Some(result), ownership: ResultAvailable, cleanup },
  }
  let owned : OwnedInvocationResult = { context: self, state }
  self.register_cleanup(fn() { owned.drop_if_available() })
  owned
}

///|
fn OwnedInvocationResult::drop_if_available(
  self : OwnedInvocationResult,
) -> Unit {
  if self.state.val.ownership == ResultAvailable ||
    self.state.val.ownership == ResultProjected {
    let result = self.state.val.result
    self.state.val.result = None
    self.state.val.ownership = ResultDropped
    match result {
      Some(result) => (self.state.val.cleanup)(result)
      None => ()
    }
  }
}

///|
fn OwnedInvocationResult::take(
  self : OwnedInvocationResult,
  target : InvocationResultOwnershipState,
) -> Result[@toolCore.InvocationResult, String] {
  if self.context.is_revoked() {
    self.drop_if_available()
    return Err("middleware invocation ownership has been revoked")
  }
  if self.state.val.ownership != ResultAvailable {
    return Err("underlying invocation result is no longer available")
  }
  let result = match self.state.val.result {
    Some(result) => result
    None => return Err("underlying invocation result is no longer available")
  }
  self.state.val.result = None
  self.state.val.ownership = target
  Ok(result)
}

///|
fn transfer_aware_stdout(
  source : @async-core.Stream[Byte],
) -> @async-core.Stream[Byte] {
  @async-core.Stream::produce(
    async fn(sink) {
      try {
        for ;; {
          match source.read(64) {
            Some(data) => if !sink.write_all(data[:]) { break }
            None => {
              sink.close()
              break
            }
          }
        }
      } catch {
        error => {
          @async-core.protect_from_cancel(
            async fn() { source.drop() },
            resume_on_cancel=true,
          )
          raise error
        }
      } noraise {
        _ =>
          @async-core.protect_from_cancel(
            async fn() { source.drop() },
            resume_on_cancel=true,
          )
      }
    },
    on_unstarted_drop=fn() { source.drop_sync() },
  )
}

///|
#doc(hidden)
pub fn OwnedInvocationResult::take_for_projection(
  self : OwnedInvocationResult,
) -> Result[@toolCore.InvocationResult, String] {
  if self.context.is_revoked() {
    self.drop_if_available()
    return Err("middleware invocation ownership has been revoked")
  }
  if self.state.val.ownership != ResultAvailable {
    return Err("underlying invocation result is no longer available")
  }
  let result = match self.state.val.result {
    Some(result) => result
    None => return Err("underlying invocation result is no longer available")
  }
  let projected : @toolCore.InvocationResult = {
    result: result.result,
    stdout: match result.stdout {
      Some(stdout) => Some(transfer_aware_stdout(stdout))
      None => None
    },
  }
  self.state.val.result = Some(projected)
  self.state.val.ownership = ResultProjected
  Ok(projected)
}

///|
/// Marks the result as transferred before yielding it to final guest-result
/// lowering. Generated guest integration performs the actual lowering while
/// the enclosing task group is still alive.
#doc(hidden)
pub fn OwnedInvocationResult::take_for_final(
  self : OwnedInvocationResult,
) -> Result[@toolCore.InvocationResult, String] {
  self.take(ResultTransferredFinal)
}

///|
#doc(hidden)
pub fn OwnedInvocationResult::drop(
  self : OwnedInvocationResult,
) -> Result[Unit, String] {
  if self.state.val.ownership != ResultAvailable {
    return Err("underlying invocation result is no longer available")
  }
  self.drop_if_available()
  Ok(())
}

///|
/// Revokes all copied wrappers, releases every still-available carrier, and
/// drops the raw underlying resource exactly once after active work ends.
#doc(hidden)
pub fn OwnershipContext::revoke(self : OwnershipContext) -> Unit {
  self.state.val.revoked = true
  for cleanup in self.state.val.cleanups {
    cleanup()
  }
  for acquisition in self.state.val.acquisitions {
    if acquisition.state == ResourceAvailable {
      acquisition.state = ResourceDropped
      (acquisition.drop_raw)()
    }
  }
  self.drop_raw_if_idle()
}
