#derive.agent
struct AccountAgent {
  team : String
}

fn AccountAgent::new(team : String) -> AccountAgent {
  { team, }
}

// over MCP, this method becomes a tool with this description:
/// Summarize a customer account by email //@
pub async fn AccountAgent::describe(
  self : Self, email : String,
) -> String {
  // call the CRM's MCP tool through its generated typed client:
  let call = @crm.CrmFindCustomerClient::new() //@
    .crm_find_customer(email).unwrap() //@
  let { result, .. } = call.collect()
  guard result is Ok(found) else { fail("CRM lookup failed") }
  // the MCP server's output schema types this result:
  let { name, plan } = found.structured //@
  "\{name} is on the \{plan} plan"
}
