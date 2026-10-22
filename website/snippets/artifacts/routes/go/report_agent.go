type ReportID struct{ Team string }
type BuildIn struct{ Month string }
type report struct{}

var mount = &golem.Mount{Path: "/reports/{team}",
    // Golem serves these live from this agent's own file system:
    ExposeFiles: []golem.FileMapping{
        {Route: "/", Path: "/site/index.html"}, //@
        {Route: "/*", Path: "/site/$1"}}} //@
var Reports = golem.DefineAgent[ReportID](
    golem.Spec{Name: "ReportAgent", HTTP: mount})
var Build = Reports.Method[BuildIn, golem.Unit]("build",
    golem.HTTP(golem.POST("/build/{month}"))) //@
var _ = Reports.Implement(func(id ReportID) *report {
    // the agent writes a front-end for its own team:
    golem.Must0(os.MkdirAll("/site", 0o755))
    golem.Must0(os.WriteFile("/site/index.html",
        frontEnd(id.Team), 0o644)) //@
    return &report{}
}).Handle(Build, golem.AdaptUnit(func(_ *report, in BuildIn) {
    // it is served at /report.html once this call returns:
    golem.Must0(os.WriteFile("/site/report.html",
        render(in.Month), 0o644))
}))
