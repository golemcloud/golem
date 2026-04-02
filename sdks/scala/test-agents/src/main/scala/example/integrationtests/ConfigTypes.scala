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

package example.integrationtests

import golem.config.Secret
import zio.blocks.schema.Schema

final case class DbConfig(
  host: String,
  port: Int,
  password: Secret[String]
)

object DbConfig {
  implicit val schema: Schema[DbConfig] = Schema.derived
}

final case class MyAppConfig(
  appName: String,
  apiKey: Secret[String],
  db: DbConfig
)

object MyAppConfig {
  implicit val schema: Schema[MyAppConfig] = Schema.derived
}
