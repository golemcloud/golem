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

package golem.runtime.rpc

import golem.Datetime
import golem.host.js._

private[rpc] trait RpcInvoker {
  def invokeAndAwait(functionName: String, input: JsDataValue): Either[String, JsDataValue]

  def invoke(functionName: String, input: JsDataValue): Either[String, Unit]

  def scheduleInvocation(datetime: Datetime, functionName: String, input: JsDataValue): Either[String, Unit]

  def scheduleCancelableInvocation(datetime: Datetime, functionName: String, input: JsDataValue): Either[String, CancellationToken]
}
