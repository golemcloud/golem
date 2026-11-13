@toolDefinition( //@
  // it works on the agent's files when its binding allows it:
  requiresFilesystem = true, //@
)
trait Outline:
  def outline(file: String): Headings

@toolImplementation()
final class OutlineImpl extends Outline:
  def outline(file: String): Headings =
    fs.readFileSync(file, "utf8").split('\n').toList
      .filter(_.startsWith("#"))

@agentDefinition()
trait BuildAgent extends BaseAgent:
  class Id(val project: String)
  def build(): Future[Headings]

@agentImplementation()
final class BuildAgentImpl(project: String) extends BuildAgent:
  def build(): Future[Headings] =
    for
      // bash calls the bound git, npm, tsc and node tools:
      run <- BashClient.newClient().run( //@
        "git pull && npm install && tsc && node dist/build.js", //@
        "/workspace/site", UInt(600)).map(_.orThrow)
      _ = if run.exitCode.value != 0 then sys.error(run.stderr)
      file = "/workspace/site/dist/report.md"
      headings <- OutlineClient().outline(file).map(_.orThrow)
    yield headings
