package toolstreaming

import golem.{BaseAgent, FutureInterop}
import golem.concurrent.Implicits.global
import golem.host.DurabilityApi
import golem.host.DurabilityApi.DurableFunctionType
import golem.runtime.annotations.*

import scala.concurrent.{ExecutionContext, Future, Promise}
import scala.scalajs.js
import scala.scalajs.js.Thenable.Implicits.*

/** Custom durable operations whose bodies run concurrently within one invocation. */
@agentDefinition()
trait ScalaCustomDurability extends BaseAgent {
  class Id(val name: String)

  /** An outer operation with a nested one, then a root-level sibling. */
  def nestedNoInput(): Future[List[String]]

  /** Two sibling operations with identical names and requests, started together. */
  def concurrentNoInput(): Future[List[String]]

  /** An operation waits while an ordinary root-level host call runs. */
  def rootCallWhileScopeWaits(): Future[List[String]]

  /** One operation registers a continuation that a sibling operation completes. */
  def crossScopeCompletion(): Future[List[String]]

  /** Two identical siblings; after the first one finishes, a root-level call follows it. */
  def concurrentFirstFinishes(): Future[List[String]]

  /** A failed body, then a sibling operation and a root-level call. */
  def failedBody(): Future[List[String]]

  /** Continuations run by `ExecutionContext.parasitic`, within one scope and across scopes. */
  def parasiticContinuations(): Future[List[String]]

  /** Conversions between Scala futures and JavaScript promises inside an operation. */
  def promiseConversion(): Future[List[String]]

  /** A nested operation and plain work that outlive the body that started them. */
  def escapedWork(): Future[String]

  /** An ordinary root-level host call. */
  def callback(payload: String): Future[String]
}

@agentImplementation()
final class ScalaCustomDurabilityImpl(name: String) extends ScalaCustomDurability {

  override def nestedNoInput(): Future[List[String]] =
    for {
      outer       <- operation("test-generator-parent")(generator("nested"))
      rootSibling <- generator("root-sibling")
    } yield List(outer, rootSibling)

  override def concurrentNoInput(): Future[List[String]] = {
    val slow = generator("slow")
    val fast = generator("fast")
    Future.sequence(List(slow, fast))
  }

  override def rootCallWhileScopeWaits(): Future[List[String]] = {
    val held = generator("held")
    val root = request("root")
    Future.sequence(List(held, root))
  }

  override def crossScopeCompletion(): Future[List[String]] = {
    val handoff   = Promise[Unit]()
    val receiver  = operation("test-receiver")(handoff.future.flatMap(_ => request("receiver")))
    val completer = operation("test-completer")(request("completer").map { result =>
      handoff.success(())
      result
    })
    Future.sequence(List(receiver, completer))
  }

  override def concurrentFirstFinishes(): Future[List[String]] = {
    val first  = generator("first").flatMap(result => request("release").map(_ => result))
    val second = generator("second")
    Future.sequence(List(first, second))
  }

  override def failedBody(): Future[List[String]] = {
    val failing = operation("test-failing")(
      request("fail").flatMap(_ => Future.failed(new IllegalStateException("body failed")))
    )
    failing.recover { case error => error.getMessage }.flatMap { message =>
      for {
        after <- generator("after-failure")
        root  <- request("root-after")
      } yield List(message, after, root)
    }
  }

  override def parasiticContinuations(): Future[List[String]] = {
    val handoff = Promise[Unit]()
    val inside  = operation("test-parasitic-inside")(
      request("inside").flatMap(_ => request("inside-parasitic"))(ExecutionContext.parasitic)
    )
    val receiver = operation("test-parasitic-receiver")(
      handoff.future
        .flatMap(_ => request("crossed"))(ExecutionContext.parasitic)
        .flatMap(_ => request("receiver-after"))
    )
    val completer = operation("test-parasitic-completer")(request("completer").flatMap { _ =>
      handoff.success(())
      request("completer-after")
    })
    Future.sequence(List(inside, receiver, completer))
  }

  override def promiseConversion(): Future[List[String]] = {
    val fromRoot  = jsRequest("root-promise")
    val converted = operation("test-promise")(
      FutureInterop
        .fromPromise(fromRoot)
        .flatMap(_ => request("after-root-promise"))
        .flatMap { _ =>
          val promise = FutureInterop.toPromise(request("to-promise"))
          FutureInterop.fromPromise(
            promise
              .asInstanceOf[js.Dynamic]
              .`then`((_: js.Any) => jsRequest("js-then"))
              .asInstanceOf[js.Promise[String]]
          )
        }
    )
    Future.sequence(List(converted, generator("promise-sibling")))
  }

  override def escapedWork(): Future[String] = {
    val handoff                 = Promise[Unit]()
    var escaped: Future[String] = Future.successful("")
    var child: Future[String]   = Future.successful("")
    operation("test-escape-outer") {
      operation("test-escape-inner") {
        escaped = handoff.future.flatMap(_ => request("escaped"))
        child = operation("test-escape-child")(request("child"))
        Future.successful("inner")
      }.flatMap { inner =>
        handoff.success(())
        for {
          escapedResult <- escaped
          childResult   <- child
        } yield s"$inner,$escapedResult,$childResult"
      }
    }
  }

  override def callback(payload: String): Future[String] =
    request(payload)

  private def generator(payload: String): Future[String] =
    operation("test-generator")(request(payload))

  private def operation(function: String)(body: => Future[String]): Future[String] =
    DurabilityApi.durableAsync("golem-it", function, DurableFunctionType.WriteRemote, ())(body)

  private def jsRequest(payload: String): js.Promise[String] = {
    val port = js.Dynamic.global.process.env.PORT.asInstanceOf[String]
    js.Dynamic.global
      .fetch(s"http://localhost:$port/callback?payload=$payload")
      .`then`((response: js.Dynamic) => response.text())
      .asInstanceOf[js.Promise[String]]
  }

  private def request(payload: String): Future[String] = {
    val port = js.Dynamic.global.process.env.PORT.asInstanceOf[String]
    js.Dynamic.global
      .fetch(s"http://localhost:$port/callback?payload=$payload")
      .asInstanceOf[js.Promise[js.Dynamic]]
      .toFuture
      .flatMap(response => response.text().asInstanceOf[js.Promise[String]].toFuture)
  }
}
