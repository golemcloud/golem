type NotebookID struct{ Owner string }
type AddIn struct {
    Text   string
    Caller golem.Principal
}
type notebook struct {
    owner string
    notes []string
}

var Notebook = golem.DefineAgent[NotebookID](golem.Spec{
    Name: "NotebookAgent",
    HTTP: &golem.Mount{Path: "/notes/{owner}",
        // with PKCE configured, require a one-hour Golem token:
        Auth: true, //@
        // allow this front-end origin through CORS on these routes:
        CORS: []string{"https://app.example.com"}},
})
var Add = Notebook.Method[AddIn, golem.Unit]("add",
    golem.HTTP(golem.POST("/add")))
var _ = Notebook.Implement(func(id NotebookID) *notebook {
    return &notebook{owner: id.Owner}
}).Handle(Add, golem.AdaptUnit(func(n *notebook, in AddIn) {
    // the login says who is calling; your code decides access:
    p, ok := in.Caller.(golem.OidcPrincipal)
    if !ok || p.Sub != n.owner {
        panic("not your notes")
    }
    n.notes = append(n.notes, in.Text)
}))
