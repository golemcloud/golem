#derive.agent
#derive.mount("/notes/{owner}")
// with PKCE configured, require a one-hour Golem token:
#derive.mount_auth(true) //@
// allow this front-end origin through CORS on these routes:
#derive.mount_cors("https://app.example.com")
struct NotebookAgent {
  owner : String
  notes : Array[String]
}

fn NotebookAgent::new(owner : String) -> NotebookAgent {
  { owner, notes: [] }
}

#derive.endpoint(post="/add")
pub fn NotebookAgent::add(
  self : Self,
  text : String,
  caller : @common.Principal,
) -> Unit raise {
  // the login says who is calling; your code decides access:
  guard caller is Oidc({ sub, .. }) && sub == self.owner else {
    fail("not your notes")
  }
  self.notes.push(text)
}
