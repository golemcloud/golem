#derive.config
struct SupportConfig {
  // the agent receives an opaque handle, not the key:
  stripe_key : @config.Secret[String] //@
}

#derive.agent
struct SupportAgent {
  ticket : String
  config : SupportConfig
}

fn SupportAgent::new(
  ticket : String, config : @config.Config[SupportConfig],
) -> SupportAgent {
  { ticket, config: config.value }
}

pub async fn SupportAgent::issue_refund(
  self : Self, charge : String, amount : Int,
) -> RefundResult {
  // refund needs a grant to reveal it; each reveal is recorded: //@
  // a binding can let a tool forward handles but reveal none: //@
  RefundClient::new().refund(
    charge, amount,
    @secrets.SecretHandle::from_config(self.config.stripe_key), //@
  ).unwrap()
}
