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

package golem.runtime.guest

import golem.Principal
import golem.host.js.PrincipalConverter

import scala.scalajs.js
import scala.scalajs.js.annotation.JSImport

/** Scala.js facade for `golem:agent/invocation-context@2.0.0`. */
private[runtime] object InvocationContextHost {

  @js.native
  private trait JsCaller extends js.Object {
    def original: js.Dynamic  = js.native
    def immediate: js.Dynamic = js.native
  }

  @js.native
  @JSImport("golem:agent/invocation-context@2.0.0", JSImport.Namespace)
  private object InvocationContextModule extends js.Object {
    def caller(): JsCaller = js.native
  }

  /** The principal that made the running invocation. */
  def immediatePrincipal(): Principal =
    PrincipalConverter.fromJs(InvocationContextModule.caller().immediate)
}
