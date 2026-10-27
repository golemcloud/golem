import golem.bridge.client.crm_find_customer.CrmFindCustomerClient

@agentDefinition()
trait AccountAgent extends BaseAgent:
  class Id(val team: String)
  // over MCP, this method becomes a tool with this description:
  @description("Summarize a customer account by email") //@
  def describe(email: String): Future[String]

@agentImplementation()
final class AccountAgentImpl(team: String) extends AccountAgent:
  def describe(email: String): Future[String] =
    // a client generated from the MCP server's schemas:
    // Golem checks, records and replays it like any tool call:
    // a retry resends the same idempotency key to the server:
    val call = CrmFindCustomerClient.newClient() //@
      .crmFindCustomer(email).orThrow //@
    call.collect().map: collected =>
      // the MCP server's output schema types this result:
      val found = collected.result.orThrow.structured //@
      s"${found.name} is on the ${found.plan} plan"
