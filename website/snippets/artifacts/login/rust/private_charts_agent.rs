#[derive(AllowedMimeTypes, Clone, Debug)]
pub enum PngType { #[mime_type("image/png")] Png }
type Png = UnstructuredBinary<PngType>;
#[agent_definition(mount = "/private/{team}",
  // Golem checks sign-in before the HTTP call reaches the agent:
  auth = true)] //@
pub trait PrivateChartsAgent {
  fn new(team: String) -> Self;
  #[endpoint(post = "/ask")]
  async fn ask(&mut self, question: String, caller: Principal)
    -> Result<u32, AppError>;
  #[endpoint(get = "/charts/{n}")]
  fn open(&self, n: u32, caller: Principal) -> Option<Png>;
}
struct PrivateChartsAgentImpl { charts: Vec<Chart> }
#[agent_implementation]
impl PrivateChartsAgent for PrivateChartsAgentImpl {
  fn new(_team: String) -> Self { Self { charts: vec![] } }
  async fn ask(&mut self, question: String, caller: Principal)
    -> Result<u32, AppError> {
    // the agent charts a signed-in user's question for that user:
    let png = answer_with_chart(&question).await?;
    self.charts.push(Chart { asked_by: user_key(&caller)?, png }); //@
    Ok(self.charts.len() as u32)
  }
  fn open(&self, n: u32, caller: Principal) -> Option<Png> {
    let made = self.charts.get(n.checked_sub(1)? as usize)?;
    if made.asked_by != user_key(&caller).ok()? { return None; } //@
    Some(Png::from_inline(made.png.clone(), PngType::Png))
  }
}
