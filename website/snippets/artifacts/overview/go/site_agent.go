type SiteID struct{ Project string }
type GenerateIn struct{ Brief string }
type site struct{}

var Site = golem.DefineAgent[SiteID](golem.Spec{
    Name: "SiteAgent",
    HTTP: &golem.Mount{Path: "/sites/{project}",
        ExposeFiles: []golem.FileMapping{
            {Route: "/", Path: "/site/index.html"}}}, //@
})
var Generate = Site.Method[GenerateIn, golem.AgentStream[string]]( //@
    "generate", golem.HTTP(golem.POST("/generate")))
var _ = Site.Implement(func(SiteID) *site { return &site{} }).
    Handle(Generate, golem.Adapt((*site).generate))

func (*site) generate(in GenerateIn) golem.AgentStream[string] {
    return golem.ProduceStream(func(
        w *golem.AgentStreamWriter[string]) error {
        page := ""
        for part := range streamPage(in.Brief) {
            page += part
            if err := w.Write(part); err != nil {
                return err
            }
        }
        golem.Must0(os.MkdirAll("/site", 0o755))
        // it's served at /sites/{project} once this call completes:
        return os.WriteFile("/site/index.html", []byte(page), 0o644) //@
    })
}
