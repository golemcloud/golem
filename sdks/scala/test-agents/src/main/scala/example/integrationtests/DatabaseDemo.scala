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

package example.integrationtests

import golem.runtime.annotations.{agentDefinition, description}
import golem.BaseAgent

import scala.concurrent.Future

@agentDefinition()
@description("Demonstrates typed Postgres and MySQL queries with fully typed parameters and results.")
trait DatabaseDemo extends BaseAgent {

  class Id(val value: String)

  @description("Run typed Postgres queries with PostgresDbValue params and result reading.")
  def postgresDemo(): Future[String]

  @description("Run typed MySQL queries with MysqlDbValue params and result reading.")
  def mysqlDemo(): Future[String]

  @description("Construct representative values from every major RDBMS type category.")
  def typeShowcase(): Future[String]
}
