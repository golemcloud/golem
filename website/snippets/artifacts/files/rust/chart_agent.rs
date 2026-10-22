#[derive(AllowedMimeTypes, Clone, Debug)]
pub enum PngType { #[mime_type("image/png")] Png }
type Png = UnstructuredBinary<PngType>;

#[agent_definition(mount = "/charts/{team}")]
pub trait ChartAgent {
  fn new(team: String) -> Self;
  // over MCP, a chat assistant calls this when a user asks:
  #[description("Draw a sales chart for one quarter")]
  #[endpoint(get = "/chart.png?quarter={quarter}")]
  async fn chart(&self, quarter: String)
    // the reply is the PNG itself, over HTTP or MCP:
    -> Result<Png, AppError>; //@
}
struct ChartAgentImpl { team: String }
#[agent_implementation]
impl ChartAgent for ChartAgentImpl {
  fn new(team: String) -> Self { Self { team } }
  async fn chart(&self, quarter: String)
    -> Result<Png, AppError> {
    // the chart is drawn for this call, and no file is written:
    let rows = query_sales(&self.team, &quarter).await?;
    let data = draw_chart(&rows); //@
    Ok(Png::from_inline(data, PngType::Png)) //@
  }
}
