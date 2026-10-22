#derive.agent
// the path names the agent: /dashboards/ana is Ana's own agent:
#derive.mount("/dashboards/{user}") //@
#derive.expose_files("/*", "/out/$1")
struct DashboardAgent { user : String; mut made : Int }

// the first request to /dashboards/ana creates her agent:
fn DashboardAgent::new(user : String) -> DashboardAgent { //@
  { user, made: 0 }
}

#derive.endpoint(post="/ask")
pub async fn DashboardAgent::ask(
  self : Self, request : String,
) -> String {
  self.made += 1
  let n = self.made
  let root = @fs.get_root_dir().unwrap()
  root.create_directory_at("out") |> ignore
  // the model builds this dashboard for her one request:
  @fs.write_string(root, "/out/\{n}.html",
    dashboard_for(self.user, request)).unwrap()
  // what it made for her is served under her path:
  "/dashboards/\{self.user}/\{n}.html" //@
}
