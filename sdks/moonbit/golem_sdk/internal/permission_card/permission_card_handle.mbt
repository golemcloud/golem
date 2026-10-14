// Opaque, affine carriers for owned `golem:core/types@2.0.0` `permission-card`
// resources, and the registry of the raw handles they hold.
//
// This package is internal to the `golemcloud/golem_sdk` module: MoonBit only
// lets packages of this module import it, and calling a method of a type needs
// an import of the type's package. Code outside the SDK can keep a holder it
// receives (for example from `SchemaValue::PermissionCard`) but cannot create
// one, take or borrow its raw handle, or call any other operation on it.
//
// The registry records the raw handle of every present holder. A decode path
// lifts a raw handle into a holder only when no present holder owns it, so data
// built by user code cannot alias a card that the agent holds through the SDK.
// The generated raw `@types.PermissionCard` is a public `Int` wrapper, so user
// code can still name a raw handle that no holder owns. The host checks every
// handle: an unknown handle traps, and a known handle carries only authority
// that the agent already has.

///|
/// The present holders, by raw handle index. Each entry empties its holder
/// without releasing the raw handle.
let held : Map[Int, () -> Unit] = Map([])

///|
fn register(raw : @types.PermissionCard, invalidate : () -> Unit) -> Unit {
  // A host call returns only handles that the agent does not hold. A holder
  // that still names the index lost its handle outside the SDK, so it is
  // emptied.
  if held.get(raw.0) is Some(previous) {
    previous()
  }
  held[raw.0] = invalidate
}

///|
fn unregister(raw : @types.PermissionCard) -> Unit {
  held.remove(raw.0)
}

///|
/// Whether a present holder owns `raw`.
pub fn is_held(raw : @types.PermissionCard) -> Bool {
  held.contains(raw.0)
}

///|
/// Drops `raw` unless a present holder owns it. Use this for raw handles found
/// in a wire tree, which can name a handle that a card holds.
pub fn drop_unless_held(raw : @types.PermissionCard) -> Unit {
  if !is_held(raw) {
    raw.drop()
  }
}

///|
/// Opaque, affine (take-once) holder for an owned persistent `permission-card`
/// resource handle. The guest can hold and transfer the card, but cannot
/// inspect or forge the runtime-owned authority represented by the raw
/// resource.
pub struct GuestPermissionCardHandle {
  priv mut cell : @types.PermissionCard?
}

///|
/// Wraps a handle that a host call returned. Use this only for a handle that
/// comes directly from the host, never for a handle read from a wire tree.
pub fn adopt(raw : @types.PermissionCard) -> GuestPermissionCardHandle {
  let handle : GuestPermissionCardHandle = { cell: Some(raw), }
  register(raw, () => handle.cell = None)
  handle
}

///|
/// Wraps a handle read from a wire tree, or returns `None` when a present
/// holder already owns it.
pub fn lift(raw : @types.PermissionCard) -> GuestPermissionCardHandle? {
  if is_held(raw) {
    None
  } else {
    Some(adopt(raw))
  }
}

///|
/// Moves the owned handle out, leaving the holder empty.
pub fn take(handle : GuestPermissionCardHandle) -> @types.PermissionCard? {
  let current = handle.cell
  if current is Some(raw) {
    unregister(raw)
  }
  handle.cell = None
  current
}

///|
/// The owned handle, borrowed. The holder keeps it.
pub fn borrow(handle : GuestPermissionCardHandle) -> @types.PermissionCard? {
  handle.cell
}

///|
/// Whether the handle is still present (not yet transferred).
pub fn GuestPermissionCardHandle::is_present(
  self : GuestPermissionCardHandle,
) -> Bool {
  self.cell is Some(_)
}

///|
/// Identity-based equality: two holders are equal only if they are the same
/// instance. An opaque capability has no structural content to compare.
pub impl Eq for GuestPermissionCardHandle with fn equal(self, other) {
  physical_equal(self, other)
}

///|
pub impl Debug for GuestPermissionCardHandle with fn to_repr(self) {
  Repr::literal(
    if self.is_present() {
      "GuestPermissionCardHandle(present)"
    } else {
      "GuestPermissionCardHandle(consumed)"
    },
  )
}

///|
/// Opaque, affine holder for the handle of an invocation-local scope card. It
/// has no schema or wire support, so a scope card cannot travel as an agent or
/// tool value.
pub struct ScopeCardHandle {
  priv mut cell : @types.PermissionCard?
}

///|
/// Wraps a scope-card handle that `derive-scope` returned.
pub fn adopt_scope(raw : @types.PermissionCard) -> ScopeCardHandle {
  let handle : ScopeCardHandle = { cell: Some(raw), }
  register(raw, () => handle.cell = None)
  handle
}

///|
/// Moves the scope-card handle out, leaving the holder empty.
pub fn take_scope(handle : ScopeCardHandle) -> @types.PermissionCard? {
  let current = handle.cell
  if current is Some(raw) {
    unregister(raw)
  }
  handle.cell = None
  current
}

///|
/// The scope-card handle, borrowed. The holder keeps it.
pub fn borrow_scope(handle : ScopeCardHandle) -> @types.PermissionCard? {
  handle.cell
}

///|
/// Whether the scope-card handle is still present.
pub fn ScopeCardHandle::is_present(self : ScopeCardHandle) -> Bool {
  self.cell is Some(_)
}
