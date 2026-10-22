// it works on the agent's files when its binding allows it:
#derive.tool("outline", requires_filesystem=true) //@
struct Outline {}

// each call runs in a fresh WebAssembly VM:
pub async fn Outline::outline(file : String) -> Array[String] {
  let root = @fs.get_root_dir().unwrap()
  let text = @fs.read_string(root, file).unwrap()
  let lines = text.split("\n").map(line => line.to_owned())
  lines.filter(line => line.has_prefix("#")).collect()
}

#derive.agent
struct BuildAgent {
  project : String
}

fn BuildAgent::new(project : String) -> BuildAgent {
  { project, }
}

pub async fn BuildAgent::build(self : Self) -> Array[String] {
  // bash calls the bound git, npm, tsc and node tools:
  let run = @bash.BashClient::new().run( //@
    "git pull && npm install && tsc && node dist/build.js", //@
    "/workspace/site", 600,
  ).unwrap()
  if run.exit_code != 0 { fail(run.stderr) }
  let file = "/workspace/site/dist/report.md"
  OutlineClient::new().outline(file).unwrap()
}
