///|
fn multipart_agent_source(
  loader : String,
  saver? : Bool = true,
  byte_hooks? : Bool = false,
) -> String {
  let declaration =
    #|#derive.agent(snapshotting="every_n(3)")
    #|pub(all) struct Index { revision : Int } derive(ToJson, FromJson)
    #|pub fn Index::new() -> Index { { revision: 0 } }
    #|pub fn Index::get(self : Self) -> Int { self.revision }
  let save = if saver {
    (
      #|impl @agents.MultipartSnapshottable for Index with save_snapshot_parts(self) -> Result[@agents.MultipartSnapshot, String] { ... }
    )
  } else {
    ""
  }
  let bytes = if byte_hooks {
    (
      #|impl @agents.Snapshottable for Index with save_snapshot(self) -> Bytes { ... }
      #|pub fn Index::load_snapshot(bytes : Bytes, context : @agents.SnapshotRestoreContext) -> Result[Index, String] { ... }
    )
  } else {
    ""
  }
  declaration + "\n" + save + "\n" + loader + "\n" + bytes
}

///|
test "multipart generator specializes transport suppresses automatic JSON and hides hooks" {
  let loader = "pub async fn Index::load_snapshot_parts(snapshot : @agents.MultipartSnapshot, context : @agents.SnapshotRestoreContext) -> Result[Index, String] { ... }"
  let agents = parse_agents([("index.mbt", multipart_agent_source(loader))]) catch {
    AgentParseError(message) => fail(message)
  }
  assert_true(agents[0].has_multipart_snapshottable)
  assert_true(agents[0].has_json_derives)
  assert_eq(agents[0].methods.length(), 1)
  assert_eq(agents[0].methods[0].name, "get")
  let output = @formatter.impls_to_string(generate_agent_registration(agents))
  let compact = strip_whitespace(output)
  assert_true(compact.contains("SnapshotData::Multipart(parts)"))
  assert_true(compact.contains("Index::load_snapshot_parts(parts,context)"))
  assert_true(compact.contains("multipart_snapshottable:Some(agent)"))
  assert_true(!output.contains("impl @agents.Snapshottable"))
  assert_true(!output.contains("SnapshotFormat::Json"))
  guard output.split_once("load_snapshot: Some") is Some((_, restoration)) else {
    fail("missing loader")
  }
  assert_true(!restoration.to_owned().contains("Index::new"))
}

///|
test "multipart generator rejects incomplete conflicting and invalid loader signatures" {
  let valid = "pub async fn Index::load_snapshot_parts(snapshot : @agents.MultipartSnapshot, context : @agents.SnapshotRestoreContext) -> Result[Index, String] { ... }"
  let cases = [
    multipart_agent_source(""),
    multipart_agent_source(valid, saver=false),
    multipart_agent_source(valid, byte_hooks=true),
    multipart_agent_source(
      "pub fn Index::load_snapshot_parts(snapshot : @agents.MultipartSnapshot, context : @agents.SnapshotRestoreContext) -> Result[Index, String] { ... }",
    ),
    multipart_agent_source(
      "pub async fn Index::load_snapshot_parts(self : Self, context : @agents.SnapshotRestoreContext) -> Result[Index, String] { ... }",
    ),
    multipart_agent_source(
      "pub async fn Index::load_snapshot_parts(snapshot : Bytes, context : @agents.SnapshotRestoreContext) -> Result[Index, String] { ... }",
    ),
    multipart_agent_source(
      "pub async fn Index::load_snapshot_parts(snapshot~ : @agents.MultipartSnapshot, context : @agents.SnapshotRestoreContext) -> Result[Index, String] { ... }",
    ),
    multipart_agent_source(
      "pub async fn Index::load_snapshot_parts(snapshot : @agents.MultipartSnapshot, context : @agents.SnapshotRestoreContext) -> Index { ... }",
    ),
  ]
  for source in cases {
    assert_true(
      capture_agent_parse_error(() => parse_agents([("index.mbt", source)]))
      is Err(_),
    )
  }
}
