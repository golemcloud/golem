/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.concurrent

import golem.host.DurabilityApi
import golem.host.DurabilityApi.DurableFunctionType
import zio.ZIO
import zio.test._

import scala.concurrent.{ExecutionContext, Future, Promise}
import scala.scalajs.js
import scala.util.Try

object CustomDurabilityScopesSpec extends ZIOSpecDefault {

  private val cell = js.Dynamic.global.__golemCustomInvocationContext
  private val host = js.Dynamic.global.__golemDurability

  private implicit val ec: ExecutionContext = GolemExecutionContext.global

  private def current(): String = cell.currentName().asInstanceOf[String]

  private def events(): List[String] = cell.events.asInstanceOf[js.Array[String]].toList

  private def root: CustomInvocationToken = cell.root.asInstanceOf[CustomInvocationToken]

  private def scope(name: String): CustomInvocationToken =
    CustomInvocationContext.run(
      root,
      () => CustomInvocationContext.enterLive(js.Dynamic.literal(functionName = name, enter = () => ()))
    )

  private def nextJobs(): Future[Unit] = {
    val done = Promise[Unit]()
    js.timers.setTimeout(0)(done.success(()))
    done.future
  }

  private def admit(name: String): Future[Unit] =
    nextJobs().map(_ => host.admit(name)).flatMap(_ => nextJobs())

  private def operation[A](name: String)(body: ExecutionContext ?=> Future[String]): Future[String] =
    DurabilityApi.durableAsync("it", name, DurableFunctionType.WriteRemote, ())(body)

  private def scenario[A](run: => Future[A]) =
    ZIO.fromFuture { _ =>
      host.reset()
      run
    }

  def spec = suite("CustomDurabilityScopesSpec")(
    test("a callback runs in the scope that registered it, not in the scope that completed its source") {
      scenario {
        val source = Promise[Unit]()
        val seen   = Promise[String]()
        val a      = scope("A")
        val b      = scope("B")
        CustomInvocationContext.run(a, () => source.future.foreach(_ => seen.success(current())))
        CustomInvocationContext.run(b, () => source.success(()))
        seen.future.map(name => (name, current()))
      }.map { case (name, after) => assertTrue(name == "A", after == "root") }
    },
    test("a task given to execute runs in the submitter's scope") {
      scenario {
        val seen = Promise[String]()
        val a    = scope("A")
        CustomInvocationContext.run(a, () => GolemExecutionContext.global.execute(() => seen.success(current())))
        seen.future
      }.map(name => assertTrue(name == "A"))
    },
    test("an inline task restores the running scope, also when it throws") {
      val inline = new ExecutionContext {
        def execute(runnable: Runnable): Unit     = Try(runnable.run())
        def reportFailure(cause: Throwable): Unit = ()
      }
      scenario {
        val b       = scope("B")
        val a       = scope("A")
        val underB  = CustomInvocationContext.run(b, () => GolemExecutionContext.wrap(inline).prepare())
        var inside  = ""
        val outside = CustomInvocationContext.run(
          a,
          () => {
            underB.execute { () =>
              inside = current()
              throw new RuntimeException("task failed")
            }
            current()
          }
        )
        Future.successful((inside, outside))
      }.map { case (inside, outside) => assertTrue(inside == "B", outside == "A") }
    },
    test("siblings admitted in the opposite order each finish and drop in their own scope") {
      scenario {
        val slow = operation("slow")(Future.successful("slow"))
        val fast = operation("fast")(Future.successful("fast"))
        for {
          _       <- admit("it::fast")
          _       <- admit("it::slow")
          results <- Future.sequence(List(slow, fast))
        } yield (results, events(), current())
      }.map { case (results, log, after) =>
        assertTrue(
          results == List("slow", "fast"),
          log == List(
            "begin it::slow in root",
            "begin it::fast in root",
            "enter it::fast in root",
            "finish it::fast in it::fast",
            "drop it::fast in it::fast",
            "close it::fast",
            "enter it::slow in root",
            "finish it::slow in it::slow",
            "drop it::slow in it::slow",
            "close it::slow"
          ),
          after == "root"
        )
      }
    },
    test("a parent finishes only after a nested operation that outlives its body") {
      scenario {
        val release = Promise[String]()
        var child   = Future.successful("")
        val parent  = operation("parent") {
          child = operation("child")(release.future)
          Future.successful("parent")
        }
        for {
          _            <- admit("it::parent")
          _            <- admit("it::child")
          beforeRelease = events()
          _             = release.success("child")
          results      <- Future.sequence(List(parent, child))
        } yield (results, beforeRelease, events())
      }.map { case (results, beforeRelease, log) =>
        assertTrue(
          results == List("parent", "child"),
          beforeRelease.contains("begin it::child in it::parent"),
          !beforeRelease.exists(_.startsWith("finish it::parent")),
          log.takeRight(6) == List(
            "finish it::child in it::child",
            "drop it::child in it::child",
            "close it::child",
            "finish it::parent in it::parent",
            "drop it::parent in it::parent",
            "close it::parent"
          )
        )
      }
    },
    test("a failed body drops the operation in its own scope and does not finish it") {
      scenario {
        val failed = operation("failing")(Future.failed(new RuntimeException("body failed")))
        admit("it::failing").flatMap(_ => failed.transform(result => Try((result.failed.get.getMessage, events()))))
      }.map { case (message, log) =>
        assertTrue(
          message == "body failed",
          log == List(
            "begin it::failing in root",
            "enter it::failing in root",
            "drop it::failing in it::failing",
            "close it::failing"
          )
        )
      }
    },
    test("an asynchronous operation cannot start inside a synchronous durable body") {
      scenario {
        var nested = Future.successful("")
        DurabilityApi.durable("it", "sync", DurableFunctionType.WriteRemote, ()) {
          nested = operation("nested")(Future.successful("nested"))
          "sync"
        }
        nested.transform(result => Try((result.failed.get.getMessage, events())))
      }.map { case (message, log) =>
        assertTrue(
          message.contains("cannot run inside a synchronous durable body"),
          log == List("begin it::sync in root", "finish it::sync in root")
        )
      }
    },
    test("the body's execution context takes precedence over an imported one") {
      scenario(ImportedGlobal.bodyContext()).map(context => assertTrue(context.isInstanceOf[GolemExecutionContext]))
    }
  ) @@ TestAspect.sequential

  private object ImportedGlobal {
    import scala.concurrent.ExecutionContext.Implicits.global

    def bodyContext(): Future[ExecutionContext] = {
      var seen: ExecutionContext = global
      val result                 = DurabilityApi.durableAsync("it", "precedence", DurableFunctionType.WriteRemote, ()) {
        seen = summon[ExecutionContext]
        Future.successful("done")
      }
      admit("it::precedence").flatMap(_ => result).map(_ => seen)
    }
  }
}
