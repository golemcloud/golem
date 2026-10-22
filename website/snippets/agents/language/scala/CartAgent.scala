final case class Item(sku: String, quantity: Int) derives Schema

// the first call creates one durable CartAgent per user:
@agentDefinition()
trait CartAgent extends BaseAgent:
  class Id(val user: String)
  def add(item: Item): Int //@
  def checkout(): Future[String]

@agentImplementation()
final class CartAgentImpl(user: String) extends CartAgent:
  private val items = ArrayBuffer.empty[Item]

  def add(item: Item): Int =
    items += item //@
    items.length

  def checkout(): Future[String] =
    placeOrder(user, items.toList).map: order =>
      items.clear()
      order.id
