#[derive(ConfigSchema)]
pub struct SupportConfig {
  // the agent receives an opaque handle, not the key:
  #[config_schema(secret)] stripe_key: Secret<String>, //@
}
#[agent_definition]
pub trait SupportAgent {
  fn new(ticket: String,
    #[agent_config] config: Config<SupportConfig>) -> Self;
  async fn issue_refund(&self, charge: String, amount: u64)
    -> Result<Receipt, AppError>;
}
struct SupportAgentImpl { config: Config<SupportConfig> }
#[agent_implementation]
impl SupportAgent for SupportAgentImpl {
  fn new(_ticket: String,
    #[agent_config] config: Config<SupportConfig>) -> Self {
    Self { config }
  }
  async fn issue_refund(&self, charge: String, amount: u64)
    -> Result<Receipt, AppError> {
    // refund needs a grant to reveal it; each reveal is recorded: //@
    // a binding can let a tool forward handles but reveal none: //@
    let api_key = self.config.get()?.stripe_key.handle()?;
    Ok(RefundClient::new().refund(charge, amount, api_key).await?) //@
  }
}
