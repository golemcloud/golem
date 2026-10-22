struct Png {}
impl @schema.AllowedMimeTypes for Png with fn mime_types() {
  ["image/png"]
}

#derive.agent
#derive.mount("/charts/{team}")
struct ChartAgent {
  team : String
}

fn ChartAgent::new(team : String) -> ChartAgent {
  { team, }
}

// over MCP, a chat assistant calls this when a user asks:
/// Draw a sales chart for one quarter
#derive.endpoint(get="/chart.png?quarter={quarter}")
pub async fn ChartAgent::chart(
  self : Self, quarter : String,
  // the reply is the PNG itself, over HTTP or MCP:
) -> @schema.UnstructuredBinary[Png] { //@
  // the chart is drawn for this call, and no file is written:
  let bytes = draw_chart(query_sales(self.team, quarter)) //@
  Inline(bytes~, mime_type=Some("image/png")) //@
}
