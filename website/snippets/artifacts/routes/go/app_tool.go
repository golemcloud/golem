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
        name := string(a.Name)
        golem.Must0(os.MkdirAll("/out", 0o755))
        golem.Must0(os.WriteFile("/out/"+name+".html", //@
            []byte(a.HTML), 0o644))
        return name + ".html", nil
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
    Sales, golem.Adapt(func(_ *AppID, in SalesIn) []Row {
        return querySales(in.Region)
    }))
