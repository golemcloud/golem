#derive.agent
// the path names the agent: /dashboards/ana is Ana's own agent:
#derive.mount("/dashboards/{user}") //@
#derive.mount_auth(true)
#derive.expose_files("/*", "/out/$1")
struct DashboardAgent { user : String }

// the first request to /dashboards/ana creates her agent:
fn DashboardAgent::new(user : String) -> DashboardAgent { //@
  { user, }
}

#derive.endpoint(post="/ask")
pub async fn DashboardAgent::ask(
  self : Self, request : String,
) -> String {
  let name = @api.generate_idempotency_key_string()
  let root = @fs.get_root_dir().unwrap()
  root.create_directory_at("out") |> ignore
  // the model builds this dashboard for her one request:
  @fs.write_string(root, "/out/\{name}.html",
    dashboard_for(self.user, request)).unwrap()
  // what it made for her is served under her path:
  "/dashboards/\{self.user}/\{name}.html" //@
}
