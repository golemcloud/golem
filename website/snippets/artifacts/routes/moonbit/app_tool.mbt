// AppAgent's model calls this tool to save an app it just wrote:
#derive.tool("present", requires_filesystem=true)
struct Present {}

#derive.arg("name", regex="^[a-z0-9-]{1,40}$")
pub async fn Present::present(
  name : String, html : String,
) -> String {
  // the app is saved in the calling agent's own files:
  let root = @fs.get_root_dir().unwrap()
  root.create_directory_at("out") |> ignore
  @fs.write_string(root, "/out/\{name}.html", html).unwrap() //@
  "\{name}.html"
}
#derive.agent
#derive.mount("/apps/{chat}")
#derive.expose_files("/*", "/out/$1")
struct AppAgent { chat : String }

fn AppAgent::new(chat : String) -> AppAgent {
  { chat, }
}

// the app's buttons fetch('sales?region=...') from here:
#derive.endpoint(get="/sales?region={region}") //@
pub async fn AppAgent::sales(
  self : Self, region : String,
) -> Array[Row] {
  // each click runs this as durable code in the agent's own VM:
  query_sales(region) //@
}
