type PageID struct{ Page string }
type PublishIn struct{ HTML string }
type page struct{ html string }

var Page = golem.DefineAgent[PageID](golem.Spec{
    Name: "PageAgent",
    HTTP: &golem.Mount{Path: "/pages/{page}"},
})
var (
    Publish = Page.Method[PublishIn, golem.Unit]("publish",
        golem.HTTP(golem.PUT("/")))
    Read = Page.Method[golem.Unit, string]("read",
        // outgoing HTTP, RPC, key-value and blob writes trap:
        // publish clears the cache when it succeeds:
        // cache hits do not wake the agent:
        golem.ReadOnly(), //@
        golem.HTTP(golem.GET("/")))
)

var impl = Page.Implement(func(PageID) *page { return &page{} })
var _ = impl.Handle(Publish, golem.AdaptUnit((*page).publish))
var _ = impl.Handle(Read, golem.Adapt0((*page).read))

func (p *page) publish(in PublishIn) { p.html = in.HTML }
func (p *page) read() string         { return p.html }
