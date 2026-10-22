var Outline = tool.DefineTool[Outliner]("outline", tool.Spec{ //@
    // it works on the agent's files when its binding allows it:
    RequiresFilesystem: true, //@
}).Body[Args, Headings](func(a *Args, s *tool.CommandSpec) {
    s.Positional(&a.File)
})
var _ = Outline.Handle(
    func(_ *tool.Context, a Args) (Headings, error) {
        text := string(golem.Must(os.ReadFile(a.File)))
        headings := regexp.MustCompile("(?m)^#.*")
        return headings.FindAllString(text, -1), nil
    })

var Builder = golem.DefineAgent[BuildID](
    golem.Spec{Name: "BuildAgent"})
var Build = Builder.Method[golem.Unit, Headings]("build")
var _ = Builder.Implement(func(id BuildID) *BuildID {
    return &id
}).Handle(Build, golem.Adapt0(func(*BuildID) Headings {
    // bash calls the bound git, npm, tsc and node tools:
    script := "git pull && npm install && tsc && node dist/build.js" //@
    run := golem.Must(bash.Run.Call(func(a *bash.RunArgs) { //@
        a.Script, a.Cwd, a.Timeout = script, "/workspace/site", 600
    }))
    if run.ExitCode != 0 {
        panic(run.Stderr)
    }
    file := "/workspace/site/dist/report.md"
    return golem.Must(Outline.Call(func(a *Args) { a.File = file }))
}))
