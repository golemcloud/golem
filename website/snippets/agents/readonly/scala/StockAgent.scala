@agentDefinition()
trait StockAgent extends BaseAgent:
  class Id(val sku: String)
  def restock(n: Int): Unit
  // repeat calls come from a cache, without loading the agent:
  // the cache clears when a restock succeeds:
  // outgoing HTTP, agent calls and key-value writes fail:
  @readOnly //@
  def level(): Int

@agentImplementation()
final class StockAgentImpl(sku: String) extends StockAgent:
  private var units = 0

  def restock(n: Int): Unit = units += n
  def level(): Int = units
