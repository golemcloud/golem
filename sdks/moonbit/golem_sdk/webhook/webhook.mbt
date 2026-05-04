///|
/// A handle to a registered webhook. Holds the public URL (to share with
/// external systems) and the underlying promise (to wait for the incoming
/// POST request).
///
/// Typical usage:
/// ```
/// let webhook = @webhook.create()
/// let url = webhook.url()        // share this URL externally
/// let payload = webhook.wait()   // blocks until POST arrives
/// let body = payload.text()      // decode the POST body
/// ```
pub(all) struct WebhookHandler {
  priv url : String
  priv promise_id : @types.PromiseId
}

///|
/// The payload of an incoming webhook POST request.
pub(all) struct WebhookRequestPayload {
  priv payload : Bytes
}

///|
/// Creates a new webhook. Allocates a Golem promise and registers a public
/// HTTP endpoint for it. When the returned URL receives a POST request,
/// the promise is completed with the request body.
///
/// **Constraints:**
/// - The agent type must be deployed via an HTTP API mount.
/// - Only the agent type that created the promise may register a webhook for it.
pub fn create() -> WebhookHandler {
  let promise_id = @api.create_promise()
  let url = @agent_host.create_webhook(promise_id)
  { url, promise_id }
}

///|
/// Returns the public webhook URL. Share this with the external system
/// that should POST to trigger the webhook.
pub fn WebhookHandler::url(self : WebhookHandler) -> String {
  self.url
}

///|
/// Blocks until the webhook receives a POST request and returns the payload.
/// The agent is durably suspended by the Golem runtime while waiting — it
/// survives failures, restarts, and updates transparently.
pub fn WebhookHandler::wait(self : WebhookHandler) -> WebhookRequestPayload {
  let data = @api.await_promise(self.promise_id)
  { payload: data }
}

///|
/// Returns the raw bytes of the POST request body.
pub fn WebhookRequestPayload::bytes(self : WebhookRequestPayload) -> Bytes {
  self.payload
}

///|
/// Decodes the POST request body as a UTF-8 string.
/// Invalid UTF-8 sequences are replaced with the Unicode replacement character.
pub fn WebhookRequestPayload::text(self : WebhookRequestPayload) -> String {
  @utf8.decode_lossy(self.payload[:])
}
