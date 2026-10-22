// define a grant for connections to the orders API only:
fn orders_api() -> PatternGrant {
  PatternGrant::new(
    "network", "", "*", "connect", "orders.example.com:443", //@
  )
}

pub fn share_orders(target: AgentId)
  -> Result<PersistentCard, AppError> {
  let bounds = Bounds {
    lower_positive: vec![orders_api()], ..Bounds::default()
  };
  let parent = permissions::derive_from_wallet(&bounds, None)?;
  // install a one-hour child; keep its parent for revocation:
  let hour = Lifespan::iso8601("PT1H")?; //@
  let child = parent.derive(&bounds, Some(&hour))?; //@
  child.install(&Holder::Agent(target))?; //@
  Ok(parent)
}

pub fn revoke_orders(parent: PersistentCard)
  -> Result<(), AppError> {
  // revoking it revokes the child and every card derived from it:
  parent.revoke()?; //@
  Ok(())
}
