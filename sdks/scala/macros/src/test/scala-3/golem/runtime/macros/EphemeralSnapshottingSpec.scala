// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
package golem.runtime.macros

import golem.runtime.Snapshotting
import golem.runtime.annotations.*
import zio.test.*

object EphemeralSnapshottingSpec extends ZIOSpecDefault {
  @agentDefinition(mode = DurabilityMode.Ephemeral, snapshotting = "disabled")
  trait EphemeralWithoutSnapshots { class Id() }

  private val message =
    "agent 'Counter' is ephemeral and declares snapshotting, but ephemeral agents take no snapshots. " +
      "Remove the snapshotting option or make the agent durable."

  def spec = suite("EphemeralSnapshottingSpec")(
    test("generate rejects an ephemeral agent with snapshotting at compile time") {
      val errors: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        @agentDefinition(typeName = "Counter", mode = DurabilityMode.Ephemeral, snapshotting = "every(5)")
        trait A { class Id() }
        golem.runtime.macros.AgentDefinitionMacro.generate[A]
      """)
      assertTrue(errors.exists(_.message.contains(message)))
    },
    test("generateWire rejects an ephemeral agent with snapshotting at compile time") {
      val errors: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        @agentDefinition(typeName = "Counter", mode = DurabilityMode.Ephemeral, snapshotting = "enabled")
        trait A { class Id() }
        golem.runtime.macros.AgentDefinitionMacro.generateWire[A]
      """)
      assertTrue(errors.exists(_.message.contains(message)))
    },
    test("an ephemeral agent with disabled snapshotting is accepted") {
      val metadata = AgentDefinitionMacro.generate[EphemeralWithoutSnapshots]
      assertTrue(
        metadata.mode.contains("ephemeral"),
        metadata.snapshotting == Snapshotting.Disabled
      )
    }
  )
}
