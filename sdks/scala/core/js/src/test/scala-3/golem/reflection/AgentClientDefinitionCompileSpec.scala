/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 */

package golem.reflection

import scala.compiletime.testing.typeCheckErrors
import zio.test._

object AgentClientDefinitionCompileSpec extends ZIOSpecDefault {
  def spec = suite("AgentClientDefinition compile-time capabilities")(
    test("method-only and full durable definitions can get by ID") {
      val errors = typeCheckErrors("""
        import golem.reflection.*
        import golem.runtime.InputRecordCodec

        val id = golem.AgentId(java.util.UUID.randomUUID())
        AgentClientDefinition.methodOnly.getById(id)
        AgentClientDefinition.full(
          name = "Counter",
          mode = AgentMode.Durable,
          constructor = InputRecordCodec.single[String]("name")
        ).getById(id)
      """)
      assertTrue(errors.isEmpty)
    },
    test("full ephemeral definitions cannot get by ID") {
      val errors = typeCheckErrors("""
        import golem.reflection.*
        import golem.runtime.InputRecordCodec

        AgentClientDefinition.full(
          name = "Request",
          mode = AgentMode.Ephemeral,
          constructor = InputRecordCodec.single[String]("route")
        ).getById(golem.AgentId(java.util.UUID.randomUUID()))
      """)
      assertTrue(errors.nonEmpty)
    },
    test("full definitions expose upsert, create and getById factories") {
      val errors = typeCheckErrors("""
        import golem.reflection.*
        import golem.runtime.InputRecordCodec

        val durable = AgentClientDefinition.full(
          name = "Counter",
          mode = AgentMode.Durable,
          constructor = InputRecordCodec.single[String]("name")
        )
        durable.client.upsert("main")
        durable.client.create("main")
        durable.client.create("main", keyed = true, env = Map("REGION" -> "eu"))
        durable.client.getById(golem.AgentId(java.util.UUID.randomUUID()))

        val ephemeral = AgentClientDefinition.full(
          name = "Request",
          mode = AgentMode.Ephemeral,
          constructor = InputRecordCodec.single[String]("route")
        )
        ephemeral.client.upsert("live")
      """)
      assertTrue(errors.isEmpty)
    },
    test("removed identity factories and offline ID derivation do not exist") {
      val errors = List(
        typeCheckErrors("""
          import golem.reflection.*
          import golem.runtime.InputRecordCodec
          val durable = AgentClientDefinition.full("Counter", AgentMode.Durable, InputRecordCodec.single[String]("name"))
          durable.agentId("main")
        """),
        typeCheckErrors("""
          import golem.reflection.*
          import golem.runtime.InputRecordCodec
          val durable = AgentClientDefinition.full("Counter", AgentMode.Durable, InputRecordCodec.single[String]("name"))
          durable.client.get("main")
        """),
        typeCheckErrors("""
          import golem.reflection.*
          import golem.runtime.InputRecordCodec
          val durable = AgentClientDefinition.full("Counter", AgentMode.Durable, InputRecordCodec.single[String]("name"))
          durable.client.getPhantom("main", java.util.UUID.randomUUID())
        """),
        typeCheckErrors("""
          import golem.reflection.*
          import golem.runtime.InputRecordCodec
          val durable = AgentClientDefinition.full("Counter", AgentMode.Durable, InputRecordCodec.single[String]("name"))
          durable.client.newPhantom("main")
        """),
        typeCheckErrors("""golem.reflection.ParsedAgentId("Counter(main)")""")
      )
      assertTrue(errors.forall(_.nonEmpty))
    },
    test("method-only definitions do not expose lifecycle factories") {
      val errors = typeCheckErrors("""
        import golem.reflection.*

        AgentClientDefinition.methodOnly.client
      """)
      assertTrue(errors.nonEmpty)
    }
  )
}
