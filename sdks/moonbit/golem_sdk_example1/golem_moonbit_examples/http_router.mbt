///|
/// A named router with immutable files, streaming echo, and an OpenAPI provider.
#derive.http_router
#derive.mount("/site")
#derive.static_file("/assets/*", "/preferred/$1")
#derive.static_file("/assets/*", "/public/$1")
struct Site {}

///|
fn Site::new() -> Site {
  Site::{ }
}

///|
/// Forward the input incrementally; dropping an unstarted response releases it.
#derive.http_handler
pub fn Site::serve(
  _self : Self,
  request : @http.HttpRequest,
) -> @http.HttpResponse {
  let input = request.body
  {
    status: 200,
    headers: [
      {
        name: "content-type",
        value: @utf8.encode("application/octet-stream").to_array(),
      },
      { name: "set-cookie", value: @utf8.encode("first=1").to_array(), },
      { name: "set-cookie", value: @utf8.encode("second=2").to_array(), },
    ],
    body: @schema.AgentStream::produce(
      async fn(writer) {
        defer input.drop()
        for ;; {
          match input.read() {
            None => return
            Some(chunk) =>
              if writer.write_one(chunk) is @schema.PeerDropped {
                return
              }
          }
        }
      },
      on_unstarted_drop=() => input.drop(),
    ),
  }
}

///|
#derive.openapi_provider
pub fn Site::describe(_self : Self) -> String raise @http.OpenApiJsonError {
  @http.openapi_json({
    "openapi": "3.1.0",
    "info": { "title": "MoonBit streaming site", "version": "1" },
    "paths": {
      "/echo": {
        "post": {
          "operationId": "echo",
          "responses": { "200": { "description": "Incremental byte echo" } },
        },
      },
    },
  })
}

///|
/// Each durable owner creates and updates its own live file.
#derive.agent
#derive.mount("/files/{name}")
#derive.expose_files("/value", "/value.txt")
struct FileOwner {}

///|
async fn FileOwner::new(name : String) -> FileOwner {
  @fs.write_string(@fs.get_root_dir().unwrap(), "value.txt", name).unwrap()
  FileOwner::{ }
}

///|
#derive.endpoint(put="/value")
pub async fn FileOwner::update(_self : Self, value : String) -> Unit {
  @fs.write_string(@fs.get_root_dir().unwrap(), "value.txt", value).unwrap()
}

///|
/// Files-only routers do not need a synthetic handler or ordinary client.
#derive.http_router
#derive.mount("/static")
#derive.static_file("/*", "/public/$1")
struct StaticSite {}

///|
fn StaticSite::new() -> StaticSite {
  StaticSite::{ }
}
