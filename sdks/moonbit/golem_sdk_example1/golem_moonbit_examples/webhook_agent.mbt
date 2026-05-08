///|
let webhook_logger : @logging.Logger = @logging.with_name("webhook-agent")

///|
/// An agent that demonstrates webhook integration.
/// It creates a webhook URL, shares it, and then waits for an external
/// system to POST data to that URL.
#derive.agent
struct WebhookAgent {
  name : String
  mut last_payload : String
}

///|
/// Creates a new webhook agent
fn WebhookAgent::new(name : String) -> WebhookAgent {
  { name, last_payload: "" }
}

///|
/// Creates a webhook and blocks until it receives a POST request.
/// Returns the body of the POST request as a string.
///
/// Usage:
/// 1. Call this method — it returns a webhook URL and starts waiting
/// 2. POST any data to the returned URL from an external system
/// 3. The method unblocks and returns the POST body
///
/// Note: In a real scenario you would typically split this into two
/// methods (one to create and return the URL, another to await),
/// or pass the URL to another agent/system via RPC before waiting.
pub fn WebhookAgent::wait_for_webhook(self : Self) -> String {
  let webhook = @webhook.create()
  webhook_logger.info("Webhook created: " + webhook.url())
  let payload = webhook.wait()
  let text = payload.text()
  self.last_payload = text
  webhook_logger.info("Webhook received payload: " + text)
  text
}

///|
/// Returns the last received webhook payload
pub fn WebhookAgent::get_last_payload(self : Self) -> String {
  self.last_payload
}
