#derive.agent
#derive.mount("/sites/{project}")
#derive.expose_files("/", "/site/index.html") //@
struct SiteAgent {
  project : String
}

fn SiteAgent::new(project : String) -> SiteAgent {
  { project, }
}

#derive.endpoint(post="/generate")
pub fn SiteAgent::generate(
  self : Self, brief : String,
) -> @schema.AgentStream[String] { //@
  @schema.AgentStream::produce(async fn(out) {
    let mut page = ""
    let parts = stream_page(brief)
    while parts.read() is Some(part) {
      page += part
      guard out.write_one(part) is Accepted else { return }
    }
    let root = @fs.get_root_dir().unwrap()
    root.create_directory_at("site") |> ignore
    // it's served at /sites/{project} once this call completes:
    @fs.write_string(root, "/site/index.html", page).unwrap() //@
  })
}
