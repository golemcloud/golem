/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 */

package golem.runtime.tool

import scala.compiletime.testing.typeCheckErrors
import zio.test.*

object Gol40ClientCapabilityCodecAcceptanceSpec extends ZIOSpecDefault {
  override def spec = suite("GOL-40 generated-client capability codec acceptance")(
    test("CLIENT-SECRET generated clients compile for an owned secret exchange") {
      val errors: List[scala.compiletime.testing.Error] = typeCheckErrors("""
        import golem.runtime.annotations.*
        import golem.runtime.macros.WireToolMacro
        import golem.schema.GuestSecretHandle
        import golem.schema.wire.ConcreteCodec
        import golem.schema.{FromSchema, IntoSchema}

        @toolDefinition(name = "gol40-client-secret-probe", version = "1.0.0")
        trait Gol40ClientSecretProbe {
          def exchange(secret: GuestSecretHandle): GuestSecretHandle
        }

        WireToolMacro.inputGraph[Gol40ClientSecretProbe](List("exchange"))
        summon[IntoSchema[GuestSecretHandle]]
        summon[FromSchema[GuestSecretHandle]]
        summon[ConcreteCodec[GuestSecretHandle]]
        ConcreteCodec.derived[GuestSecretHandle]
      """)

      assertTrue(errors.isEmpty).label(errors.map(_.message).mkString("; "))
    },
    test("CLIENT-QUOTA generated clients compile for a named quota-token exchange") {
      val errors: List[scala.compiletime.testing.Error] = typeCheckErrors("""
        import golem.host.QuotaApi.QuotaToken
        import golem.schema.wire.ConcreteCodec

        summon[ConcreteCodec[QuotaToken]]
        ConcreteCodec.derived[QuotaToken]
      """)

      assertTrue(errors.isEmpty).label(errors.map(_.message).mkString("; "))
    },
    test("CLIENT-PERMISSION generated clients compile for a non-polymorphic permission-card exchange") {
      val errors: List[scala.compiletime.testing.Error] = typeCheckErrors("""
        import golem.schema.GuestPermissionCardHandle
        import golem.schema.wire.ConcreteCodec

        GuestPermissionCardHandle.nonPolymorphicIntoSchema
        summon[ConcreteCodec[GuestPermissionCardHandle]]
        ConcreteCodec.derived[GuestPermissionCardHandle]
      """)

      assertTrue(errors.isEmpty).label(errors.map(_.message).mkString("; "))
    }
  )
}
