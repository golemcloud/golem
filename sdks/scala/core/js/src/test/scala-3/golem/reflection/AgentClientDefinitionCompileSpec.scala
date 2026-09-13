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
    test("binding-only and complete durable definitions can bind") {
      val errors = typeCheckErrors("""
        import golem.reflection.*
        import golem.runtime.InputRecordCodec

        val id = ParsedAgentId("Counter(main)")
        AgentClientDefinition.bindingOnly.bind(id)
        AgentClientDefinition.complete(
          name = "Counter",
          mode = AgentMode.Durable,
          constructor = InputRecordCodec.single[String]("name")
        ).bind(id)
      """)
      assertTrue(errors.isEmpty)
    },
    test("complete ephemeral definitions cannot bind") {
      val errors = typeCheckErrors("""
        import golem.reflection.*
        import golem.runtime.InputRecordCodec

        AgentClientDefinition.complete(
          name = "Request",
          mode = AgentMode.Ephemeral,
          constructor = InputRecordCodec.single[String]("route")
        ).bind(ParsedAgentId("Request(live)"))
      """)
      assertTrue(errors.nonEmpty)
    }
  )
}
