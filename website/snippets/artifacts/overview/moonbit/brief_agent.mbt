#derive.agent
#derive.mount("/briefs/{client}")
// Golem serves this agent's own file at /briefs/{client}:
#derive.expose_files("/", "/out/index.html") //@
struct BriefAgent {
  client : String
}

fn BriefAgent::new(client : String) -> BriefAgent {
  { client, }
}

pub async fn BriefAgent::write(
  self : Self, notes : String,
) -> Unit {
  let html = @llm.generate_text(
    model=@llm.openai("gpt-4.1"),
    prompt="Write an HTML brief for \{self.client}:\n\{notes}",
  )
  let root = @fs.get_root_dir().unwrap()
  root.create_directory_at("out") |> ignore
  // it writes the page into its own files:
  // a read waits for this call, so it never gets half a page:
  @fs.write_string(root, "/out/index.html", html).unwrap() //@
}
