package capabilityfixture

import golem.FutureInterop
import golem.runtime.annotations.toolDefinition

import scala.scalajs.js
import scala.scalajs.js.annotation.JSExportTopLevel
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue

@toolDefinition(name = "echo", version = "1.0.0")
trait RemoteEcho {
  def echo(value: String): String
}

object EchoCaller {
  @JSExportTopLevel("callEcho")
  def callEcho(value: String): js.Promise[String] =
    FutureInterop.toPromise(RemoteEchoClient().echo(value).map {
      case Right(result) => result
      case Left(error)   => throw new IllegalStateException(error.toString)
    })
}
