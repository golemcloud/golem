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
pub using @types {type IncomingRequest}

///|
pub using @types {type OutgoingRequest}

///|
pub using @types {type RequestOptions}

///|
pub using @types {type ResponseOutparam}

///|
pub using @types {type IncomingResponse}

///|
pub using @types {type IncomingBody}

///|
pub using @types {type FutureTrailers}

///|
pub using @types {type OutgoingResponse}

///|
pub using @types {type OutgoingBody}

///|
pub using @types {type FutureIncomingResponse}

///|
pub using @streams {type InputStream}

///|
pub using @streams {type OutputStream}

///|
/// Sends an outgoing HTTP request and returns a future for the response.
pub fn handle(
  request : OutgoingRequest,
  options : RequestOptions?,
) -> Result[FutureIncomingResponse, @types.ErrorCode] {
  @outgoingHandler.handle(request, options)
}
