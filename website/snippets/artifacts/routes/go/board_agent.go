const boardPage = `<button id="add">Add</button>
<script>add.onclick = () => fetch('cards', {
  method: 'POST', body: JSON.stringify({ title: 'New card' }),
})</script>`

type BoardID struct{ Team string }
type AddIn struct{ Title string }
type board struct{ cards []string }

var Board = golem.DefineAgent[BoardID](golem.Spec{
    Name: "BoardAgent",
    // the URL names the agent: one BoardAgent per team:
    HTTP: &golem.Mount{Path: "/boards/{team}", //@
        ExposeFiles: []golem.FileMapping{
            {Route: "/app", Path: "/index.html"}}},
})
var Add = Board.Method[AddIn, []string]("add",
    // the page's Add button posts to this endpoint:
    golem.HTTP(golem.POST("/cards"))) //@
var _ = Board.Implement(func(BoardID) *board {
    golem.Must0(os.WriteFile("/index.html",
        []byte(boardPage), 0o644))
    return &board{}
}).Handle(Add, golem.Adapt(func(b *board, in AddIn) []string {
    b.cards = append(b.cards, in.Title)
    return b.cards
}))
