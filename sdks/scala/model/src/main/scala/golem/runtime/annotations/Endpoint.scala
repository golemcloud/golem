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

package golem.runtime.annotations

import java.lang.annotation.{ElementType, Retention, RetentionPolicy, Target}
import scala.annotation.StaticAnnotation

/**
 * Marks a method as an HTTP endpoint.
 *
 * Usage:
 * {{{
 * @endpoint(method = "GET", path = "/weather/{city}")
 * def getWeather(city: String): Future[WeatherReport]
 * }}}
 *
 * @param method
 *   HTTP method: GET, POST, PUT, DELETE, PATCH, HEAD, OPTIONS, etc.
 * @param path
 *   Path suffix with optional query: "/items/{id}?format={fmt}"
 * @param auth
 *   Whether authentication is required. If omitted, inherits from mount.
 * @param cors
 *   CORS allowed patterns; empty = inherit from mount
 */
@Retention(RetentionPolicy.RUNTIME)
@Target(Array(ElementType.METHOD))
final class endpoint(
  val method: String,
  val path: String,
  val auth: Boolean = false,
  val cors: Array[String] = Array.empty
) extends StaticAnnotation
