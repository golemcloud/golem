///|
/// A persistent derivation result, labelled by the host. Match the case to
/// transfer the typed card as an agent or tool value. Common operations keep
/// the host's kind without inspecting grants or interpreting request strings.
pub enum PersistentCard {
  Concrete(PermissionCard)
  Template(PolymorphicPermissionCard)
}

///|
fn PersistentCard::from_raw(raw : @raw.PersistentCard) -> PersistentCard {
  match raw {
    Concrete(raw) => Concrete(PermissionCard::from_raw(raw))
    Template(raw) => {
      let handle = @card.adopt(raw)
      ignore(@card.claim_kind(handle, true))
      Template({ handle, })
    }
  }
}

///|
pub fn PersistentCard::id(self : PersistentCard) -> CardId {
  match self {
    Concrete(card) => card.id()
    Template(card) => card.id()
  }
}

///|
pub fn PersistentCard::parents(self : PersistentCard) -> Array[CardId] {
  match self {
    Concrete(card) => card.parents()
    Template(card) => card.parents()
  }
}

///|
pub fn PersistentCard::expires_at(self : PersistentCard) -> Timestamp? {
  match self {
    Concrete(card) => card.expires_at()
    Template(card) => card.expires_at()
  }
}

///|
pub fn PersistentCard::inspect(
  self : PersistentCard,
) -> CardView raise PermissionError {
  match self {
    Concrete(card) => card.inspect()
    Template(card) => card.inspect()
  }
}

///|
pub fn PersistentCard::derive_child(
  self : PersistentCard,
  bounds : Bounds,
  lifespan? : Lifespan,
) -> PersistentCard raise PermissionError {
  match self {
    Concrete(card) => card.derive_child(bounds, lifespan?)
    Template(card) => card.derive_child(bounds, lifespan?)
  }
}

///|
/// Consumes this card even if the host rejects the revocation.
pub fn PersistentCard::revoke(
  self : PersistentCard,
) -> UInt raise PermissionError {
  match self {
    Concrete(card) => card.revoke()
    Template(card) => card.revoke()
  }
}

///|
/// Consumes this card even if the host rejects the installation.
pub fn PersistentCard::install(
  self : PersistentCard,
  target : Holder,
) -> Unit raise PermissionError {
  match self {
    Concrete(card) => card.install(target)
    Template(card) => card.install(target)
  }
}

///|
pub fn PersistentCard::is_moved(self : PersistentCard) -> Bool {
  match self {
    Concrete(card) => card.is_moved()
    Template(card) => card.is_moved()
  }
}

///|
pub fn PersistentCard::drop(self : PersistentCard) -> Unit {
  match self {
    Concrete(card) => card.drop()
    Template(card) => card.drop()
  }
}
