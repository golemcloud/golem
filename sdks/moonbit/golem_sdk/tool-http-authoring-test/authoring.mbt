///|
#derive.tool("report")
#derive.mount("/reports")
#warnings("-struct_never_constructed")
struct Report {}

///|
#derive.endpoint(post="/generate")
pub fn Report::generate() -> Unit {
  ()
}

///|
#derive.endpoint(get="/status/{id}")
pub fn Report::status(id : String) -> String {
  id
}

///|
#derive.command(subtree="Administration")
#derive.endpoint(prefix="/admin")
pub fn Report::admin() -> Unit {
  ()
}

///|
#derive.tool("administration")
#derive.mount("/discarded")
#warnings("-struct_never_constructed")
struct Administration {}

///|
#derive.endpoint(prefix="/own")
pub fn Administration::administration() -> Unit {
  ()
}

///|
#derive.endpoint(delete="/purge/{id}")
pub fn Administration::purge(id : String) -> String {
  id
}

///|
#derive.tool("retained")
#derive.mount("/retained")
#warnings("-struct_never_constructed")
struct Retained {}

///|
#derive.command(subtree="Administration")
pub fn Retained::administration() -> Unit {
  ()
}

///|
#derive.tool("http-options")
#derive.mount("/options")
#warnings("-struct_never_constructed")
struct HttpOptions {}

///|
#derive.endpoint(post="/run?format={format}", mode="durableStreams", route_id="custom")
#derive.endpoint_header("X-Token", "token")
#derive.endpoint_response_header("X-Result", "ready")
#derive.durable_stream_slot(input="stdin", name="upload", content_type="text/plain")
#derive.durable_stream_slot(output="stdout", name="download", content_type="text/csv")
#derive.durable_streams(max_concurrent_readers_per_stream="7", max_append_requests_per_second_per_stream="11")
pub fn HttpOptions::run(format : String, token : String) -> String {
  format + token
}
