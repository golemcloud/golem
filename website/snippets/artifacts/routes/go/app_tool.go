type AppID struct{ Chat string }
type SalesIn struct{ Region string }

// AppAgent's model calls this tool to save an app it just wrote:
var Present = tool.DefineTool[Presenter]("present", tool.Spec{
    RequiresFilesystem: true,
}).Body[Args, string](func(a *Args, s *tool.CommandSpec) {
    s.Positional(&a.Name)
    s.Positional(&a.HTML)
})
var _ = Present.Handle(
    func(_ *tool.Context, a Args) (string, error) {
        // the app is saved in the calling agent's own files:
        golem.Must0(os.MkdirAll("/out", 0o755))
        golem.Must0(os.WriteFile("/out/"+string(a.Name)+".html", //@
            []byte(a.HTML), 0o644))
        return string(a.Name) + ".html", nil
    })
var Apps = golem.DefineAgent[AppID](golem.Spec{
    Name: "AppAgent",
    HTTP: &golem.Mount{Path: "/apps/{chat}",
        ExposeFiles: []golem.FileMapping{
            {Route: "/*", Path: "/out/$1"}}}})
var Sales = Apps.Method[SalesIn, []Row]("sales",
    // the app's buttons fetch('sales?region=...') from here:
    golem.HTTP(golem.GET("/sales?region={region}"))) //@
var _ = Apps.Implement(func(id AppID) *AppID { return &id }).Handle(
    // each click runs this as durable code in the agent's own VM:
    Sales, golem.Adapt(func(_ *AppID, in SalesIn) []Row { //@
        return querySales(in.Region) //@
    }))
