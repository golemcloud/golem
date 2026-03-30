/*
 * Copyright 2024-2026 John A. De Goes and the ZIO Contributors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem

import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
import scala.scalajs.js
import scala.scalajs.js.JSConverters._

/**
 * Utilities for converting between Scala Futures and JavaScript Promises.
 *
 * These helpers bridge the async worlds of Scala and JavaScript, used
 * throughout the runtime for host interop.
 */
object FutureInterop {

  /**
   * Converts a JavaScript Promise to a Scala Future.
   *
   * @param promise
   *   The Promise to convert
   * @return
   *   A Future that completes with the Promise's result
   */
  def fromPromise[A](promise: js.Promise[A]): Future[A] =
    promise.toFuture

  /**
   * Converts a Scala Future to a JavaScript Promise.
   *
   * @param future
   *   The Future to convert
   * @return
   *   A Promise that resolves with the Future's result
   */
  def toPromise[A](future: Future[A]): js.Promise[A] =
    future.toJSPromise

  /**
   * Converts an Either to a Future, failing on Left.
   *
   * @param either
   *   The Either to convert
   * @return
   *   A successful Future for Right, failed Future for Left
   */
  def fromEither[A](either: Either[String, A]): Future[A] =
    either.fold(err => Future.failed(js.JavaScriptException(err)), Future.successful)

  /**
   * Creates a failed Future with the given message.
   *
   * @param message
   *   The error message
   * @return
   *   A failed Future
   */
  def failed[A](message: String): Future[A] =
    Future.failed(js.JavaScriptException(message))
}
