#derive.agent
#derive.mount("/reports/{team}")
// Golem serves these live from this agent's own file system:
#derive.expose_files("/", "/site/index.html") //@
#derive.expose_files("/*", "/site/$1") //@
struct ReportAgent {
  team : String
}

async fn ReportAgent::new(team : String) -> ReportAgent {
  // the agent writes a front-end for its own team:
  let root = @fs.get_root_dir().unwrap()
  root.create_directory_at("site") |> ignore
  let page = front_end(team)
  @fs.write_string(root, "/site/index.html", page).unwrap() //@
  { team, }
}

#derive.endpoint(post="/build/{month}") //@
pub async fn ReportAgent::build(
  self : Self, month : String,
) -> Unit {
  let root = @fs.get_root_dir().unwrap()
  let page = render(month)
  // it is served at /report.html once this call returns:
  @fs.write_string(root, "/site/report.html", page).unwrap()
}
