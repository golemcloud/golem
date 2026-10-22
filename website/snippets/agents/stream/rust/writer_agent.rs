#[agent_definition(mount = "/docs/{doc}")]
pub trait WriterAgent {
  fn new(doc: String) -> Self;
  #[endpoint(post = "/draft")]
  // clients resume this Durable Stream from an offset:
  fn draft(&self, brief: String) -> AgentStream<String>; //@
}
struct WriterAgentImpl;
#[agent_implementation]
impl WriterAgent for WriterAgentImpl {
  fn new(_doc: String) -> Self { Self }
  fn draft(&self, brief: String) -> AgentStream<String> {
    let (mut out, stream) = AgentStream::new();
    spawn_local(async move {
      for section in ["Summary", "Details", "Actions"] {
        // completed model calls are not repeated on recovery:
        let reply = text(OpenAI::send(
          OpenAiConfig::from_env().expect("no OpenAI config"),
          vec![user(&format!(
            "{brief}\nWrite the {section} section."))],
          model("gpt-4.1"),
        ).await).expect("model call failed");
        if out.write_one(reply).await.is_err() { return; } //@
      }
    });
    stream
  }
}
