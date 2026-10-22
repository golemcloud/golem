type UserID struct{ User string }
type AskIn struct{ Request string }
type dashboards struct {
    User string
    Made int
}

var Dashboards = golem.DefineAgent[UserID](golem.Spec{
    Name: "DashboardAgent",
    // the path names the agent: /dashboards/ana is Ana's own agent:
    HTTP: &golem.Mount{Path: "/dashboards/{user}", //@
        ExposeFiles: []golem.FileMapping{
            {Route: "/*", Path: "/out/$1"}}}})
var Ask = Dashboards.Method[AskIn, string]("ask",
    golem.HTTP(golem.POST("/ask")))

// the first request to /dashboards/ana creates her agent:
var _ = Dashboards.Implement(func(id UserID) *dashboards { //@
    return &dashboards{User: id.User} //@
}).Handle(Ask, golem.Adapt((*dashboards).ask))

func (d *dashboards) ask(in AskIn) string {
    d.Made++
    n := strconv.Itoa(d.Made)
    golem.Must0(os.MkdirAll("/out", 0o755))
    // the model builds this dashboard for her one request:
    golem.Must0(os.WriteFile("/out/"+n+".html",
        []byte(dashboardFor(d.User, in.Request)), 0o644))
    // what it made for her is served under her path:
    return "/dashboards/" + d.User + "/" + n + ".html" //@
}
