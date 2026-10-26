type pngType struct{}

func (pngType) MimeTypes() []string { return []string{"image/png"} }

type Png = golem.UnstructuredBinary[pngType]
type ChartID struct{ Team string }
type ChartIn struct{ Quarter string }

var Charts = golem.DefineAgent[ChartID](golem.Spec{
    Name: "ChartAgent",
    HTTP: &golem.Mount{Path: "/charts/{team}"}})
var Chart = Charts.Method[ChartIn, Png]("chart",
    // over MCP, a chat assistant calls this when a user asks:
    golem.Description("Draw a sales chart for one quarter"),
    golem.HTTP(golem.GET("/chart.png?quarter={quarter}")))
var _ = Charts.Implement(func(id ChartID) *ChartID {
    return &id
}).Handle(Chart, golem.Adapt((*ChartID).chart))

// the reply is the PNG and its MIME type, over HTTP or MCP:
func (c *ChartID) chart(in ChartIn) Png { //@
    // the chart is drawn for this call, and no file is written:
    val := drawChart(querySales(c.Team, in.Quarter)) //@
    return Png{Data: val, MimeType: "image/png"} //@
}
