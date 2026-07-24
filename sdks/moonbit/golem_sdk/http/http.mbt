///|
pub using @types {type Method}

///|
pub using @types {type Scheme}

///|
pub using @types {type DnsErrorPayload}

///|
pub using @types {type TlsAlertReceivedPayload}

///|
pub using @types {type FieldSizePayload}

///|
pub using @types {type Fields}

///|
pub using @types {type Request}

///|
pub using @types {type RequestOptions}

///|
pub using @types {type Response}

///|
pub using @types {type ErrorCode}

///|
pub using @asyncCore {type Future}

///|
pub using @asyncCore {type Stream}

///|
/// Sends an outgoing HTTP request and awaits its response.
pub async fn send(request : Request) -> Result[Response, ErrorCode] {
  @client.send(request)
}
