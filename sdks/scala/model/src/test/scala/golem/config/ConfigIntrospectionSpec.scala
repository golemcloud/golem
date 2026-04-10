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

package golem.config

import golem.data.{DataType, ElementSchema}
import zio.blocks.schema.Schema
import zio.test._

object ConfigIntrospectionSpec extends ZIOSpecDefault {

  case class SimpleConfig(name: String, count: Int)
  object SimpleConfig {
    implicit val schema: Schema[SimpleConfig] = Schema.derived
  }

  case class DbConfig(host: String, port: Int, password: Secret[String])
  object DbConfig {
    implicit val schema: Schema[DbConfig] = Schema.derived
  }

  case class NestedConfig(appName: String, db: DbConfig)
  object NestedConfig {
    implicit val schema: Schema[NestedConfig] = Schema.derived
  }

  case class AllLocalConfig(a: String, b: Int, c: Boolean, d: Long, e: Double)
  object AllLocalConfig {
    implicit val schema: Schema[AllLocalConfig] = Schema.derived
  }

  case class MultiSecretConfig(token: Secret[String], key: Secret[String], name: String)
  object MultiSecretConfig {
    implicit val schema: Schema[MultiSecretConfig] = Schema.derived
  }

  case class DeeplyNested(outer: NestedConfig)
  object DeeplyNested {
    implicit val schema: Schema[DeeplyNested] = Schema.derived
  }

  case class DbCreds(user: String, pass: String)
  object DbCreds {
    implicit val schema: Schema[DbCreds] = Schema.derived
  }

  case class SecretRecordConfig(creds: Secret[DbCreds])
  object SecretRecordConfig {
    implicit val schema: Schema[SecretRecordConfig] = Schema.derived
  }

  override def spec: Spec[TestEnvironment, Any] =
    suite("ConfigIntrospectionSpec")(
      suite("primitive root leaf")(
        test("String produces one local declaration") {
          val decls = ConfigIntrospection.declarations[String]()
          assertTrue(
            decls.size == 1,
            decls.head.source == AgentConfigSource.Local,
            decls.head.path == Nil,
            decls.head.valueType == ElementSchema.Component(DataType.StringType)
          )
        }
      ),
      suite("Secret root leaf")(
        test("Secret[String] produces one secret declaration") {
          val decls = ConfigIntrospection.declarations[Secret[String]]()
          assertTrue(
            decls.size == 1,
            decls.head.source == AgentConfigSource.Secret,
            decls.head.path == Nil,
            decls.head.valueType == ElementSchema.Component(DataType.StringType)
          )
        }
      ),
      suite("simple case class")(
        test("produces correct number of declarations") {
          val decls = ConfigIntrospection.declarations[SimpleConfig]()
          assertTrue(decls.size == 2)
        },
        test("produces local declarations for all fields") {
          val decls = ConfigIntrospection.declarations[SimpleConfig]()
          assertTrue(decls.forall(_.source == AgentConfigSource.Local))
        },
        test("field names become paths") {
          val decls = ConfigIntrospection.declarations[SimpleConfig]()
          assertTrue(
            decls.exists(d => d.path == List("name") && d.valueType == ElementSchema.Component(DataType.StringType)),
            decls.exists(d => d.path == List("count") && d.valueType == ElementSchema.Component(DataType.IntType))
          )
        }
      ),
      suite("case class with secret field")(
        test("produces correct number of declarations") {
          val decls = ConfigIntrospection.declarations[DbConfig]()
          assertTrue(decls.size == 3)
        },
        test("local fields have Local source") {
          val decls = ConfigIntrospection.declarations[DbConfig]()
          assertTrue(
            decls.exists(d => d.path == List("host") && d.source == AgentConfigSource.Local),
            decls.exists(d => d.path == List("port") && d.source == AgentConfigSource.Local)
          )
        },
        test("secret field has Secret source") {
          val decls = ConfigIntrospection.declarations[DbConfig]()
          assertTrue(
            decls.exists(d =>
              d.path == List("password") &&
                d.source == AgentConfigSource.Secret &&
                d.valueType == ElementSchema.Component(DataType.StringType)
            )
          )
        }
      ),
      suite("nested case class")(
        test("produces declarations for all leaf fields") {
          val decls = ConfigIntrospection.declarations[NestedConfig]()
          assertTrue(decls.size == 4)
        },
        test("top-level field has single-segment path") {
          val decls = ConfigIntrospection.declarations[NestedConfig]()
          assertTrue(
            decls.exists(d => d.path == List("appName") && d.source == AgentConfigSource.Local)
          )
        },
        test("nested fields have multi-segment paths") {
          val decls = ConfigIntrospection.declarations[NestedConfig]()
          assertTrue(
            decls.exists(d => d.path == List("db", "host") && d.source == AgentConfigSource.Local),
            decls.exists(d => d.path == List("db", "port") && d.source == AgentConfigSource.Local),
            decls.exists(d => d.path == List("db", "password") && d.source == AgentConfigSource.Secret)
          )
        }
      ),
      suite("path prefix propagation")(
        test("describe with prefix prepends to all paths") {
          val decls = ConfigIntrospection.declarations[SimpleConfig](List("app"))
          assertTrue(
            decls.exists(_.path == List("app", "name")),
            decls.exists(_.path == List("app", "count"))
          )
        },
        test("nested config with prefix produces correct deep paths") {
          val decls = ConfigIntrospection.declarations[NestedConfig](List("root"))
          assertTrue(
            decls.exists(_.path == List("root", "appName")),
            decls.exists(_.path == List("root", "db", "host")),
            decls.exists(_.path == List("root", "db", "port")),
            decls.exists(_.path == List("root", "db", "password"))
          )
        }
      ),
      suite("all-local config")(
        test("five fields produce five local declarations") {
          val decls = ConfigIntrospection.declarations[AllLocalConfig]()
          assertTrue(
            decls.size == 5,
            decls.forall(_.source == AgentConfigSource.Local)
          )
        },
        test("field types are correct") {
          val decls = ConfigIntrospection.declarations[AllLocalConfig]()
          assertTrue(
            decls.exists(d => d.path == List("a") && d.valueType == ElementSchema.Component(DataType.StringType)),
            decls.exists(d => d.path == List("b") && d.valueType == ElementSchema.Component(DataType.IntType)),
            decls.exists(d => d.path == List("c") && d.valueType == ElementSchema.Component(DataType.BoolType)),
            decls.exists(d => d.path == List("d") && d.valueType == ElementSchema.Component(DataType.LongType)),
            decls.exists(d => d.path == List("e") && d.valueType == ElementSchema.Component(DataType.DoubleType))
          )
        }
      ),
      suite("multiple secrets")(
        test("each secret field gets Secret source") {
          val decls = ConfigIntrospection.declarations[MultiSecretConfig]()
          assertTrue(
            decls.size == 3,
            decls.count(_.source == AgentConfigSource.Secret) == 2,
            decls.count(_.source == AgentConfigSource.Local) == 1
          )
        }
      ),
      suite("deeply nested")(
        test("three levels of nesting produce correct paths") {
          val decls = ConfigIntrospection.declarations[DeeplyNested]()
          assertTrue(
            decls.exists(_.path == List("outer", "appName")),
            decls.exists(_.path == List("outer", "db", "host")),
            decls.exists(_.path == List("outer", "db", "port")),
            decls.exists(_.path == List("outer", "db", "password"))
          )
        }
      ),
      suite("Secret[Record] stays one leaf")(
        test("Secret wrapping a case class emits one secret declaration") {
          val decls = ConfigIntrospection.declarations[SecretRecordConfig]()
          assertTrue(
            decls.size == 1,
            decls.head.source == AgentConfigSource.Secret,
            decls.head.path == List("creds")
          )
        }
      )
    )
}
