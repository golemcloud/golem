package toolstreaming

import scala.concurrent.{ExecutionContext, Future, Promise}
import scala.scalajs.js
import scala.scalajs.js.annotation.JSImport

@js.native
@JSImport("node:child_process", JSImport.Namespace)
private object NodeChildProcess extends js.Object {
  def spawnSync(command: String, args: js.Array[String]): js.Dynamic = js.native
  def execFile(
    command: String,
    args: js.Array[String],
    options: js.Object,
    callback: js.Function2[js.Any, js.Dynamic, Unit]
  ): js.Any = js.native
}

// Runs the bound `streaming` tool through node:child_process: `produce 3 2` streams chunks 0, 1
// and 2 of two bytes each, and an unknown command is a usage error from the shared parser.
private[toolstreaming] object ChildProcessTool {
  private val produce = js.Array("produce", "3", "2")

  private def hex(bytes: js.Dynamic): String =
    bytes.applyDynamic("toString")("hex").asInstanceOf[String]

  def observation()(implicit ec: ExecutionContext): Future[String] = {
    val sync = NodeChildProcess.spawnSync("streaming", produce)
    val syncSummary = s"${sync.status}:${hex(sync.stdout)}"
    val asynchronous = Promise[String]()
    NodeChildProcess.execFile(
      "streaming",
      produce,
      js.Dynamic.literal(encoding = "buffer"),
      (error: js.Any, stdout: js.Dynamic) =>
        if (error == null || js.isUndefined(error)) asynchronous.success(hex(stdout))
        else asynchronous.failure(js.JavaScriptException(error))
    )
    asynchronous.future.map { asyncSummary =>
      val usage = NodeChildProcess.spawnSync("streaming", js.Array("no-such-command"))
      s"sync=$syncSummary;async=$asyncSummary;usage=${usage.status}"
    }
  }
}
