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
  // a client generated from the MCP server's schemas:
  // Golem checks, records and replays it like any tool call:
  // a retry resends the same idempotency key to the server:
  let call = @crm.CrmFindCustomerClient::new() //@
    .crm_find_customer(email).unwrap() //@
  let { result, .. } = call.collect()
  guard result is Ok(found) else { fail("CRM lookup failed") }
  // the MCP server's output schema types this result:
  let { name, plan } = found.structured //@
  "\{name} is on the \{plan} plan"
}
