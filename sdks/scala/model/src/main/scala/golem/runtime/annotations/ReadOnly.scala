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

package golem.runtime.annotations

import java.lang.annotation.{ElementType, Retention, RetentionPolicy, Target}
import scala.annotation.StaticAnnotation

/**
 * Marks an agent method as read-only.
 *
 * A read-only method does not modify the agent's state. The platform may use
 * this information to enable caching, side-effect detection, and to expose the
 * method via HTTP GET endpoints.
 *
 * Read-only methods are not supported on `ephemeral` agents (they have no
 * shared state to read). Annotating a method of an ephemeral agent with
 * `@readOnly` is a compile-time error.
 *
 * @param cache
 *   Cache policy. Defaults to `"until-write"`. Accepted values are
 *   `"no-cache"`, `"until-write"`, or `"ttl(<duration>)"` — for example
 *   `"ttl(30 seconds)"`.
 */
@Retention(RetentionPolicy.RUNTIME)
@Target(Array(ElementType.METHOD))
final class readOnly(val cache: String = "until-write") extends StaticAnnotation
