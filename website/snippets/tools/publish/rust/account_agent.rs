use crm_find_customer_tool_guest_client::CrmFindCustomerClient;

#[agent_definition]
pub trait AccountAgent {
  fn new(team: String) -> Self;
  // over MCP, this method becomes a tool with this description:
  #[description("Summarize a customer account by email")] //@
  async fn describe(&self, email: String)
    -> Result<String, AppError>;
}
struct AccountAgentImpl;
#[agent_implementation]
impl AccountAgent for AccountAgentImpl {
  fn new(_team: String) -> Self { Self }
  async fn describe(&self, email: String)
    -> Result<String, AppError> {
    // call the CRM's MCP tool through its generated typed client:
    let call = CrmFindCustomerClient::new() //@
      .crm_find_customer(email).await?; //@
    let result = call.collect().await.result?;
    // the MCP server's output schema types this result:
    let customer = result.structured; //@
    let (name, plan) = (customer.name, customer.plan);
    Ok(format!("{name} is on the {plan} plan"))
  }
}
