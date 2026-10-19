/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 */
package golem.runtime.macros

import scala.compiletime.testing.typeCheckErrors
import zio.test._

object DurableStreamMacroCompileErrorsSpec extends ZIOSpecDefault {
  private inline def errors(inline body: String): List[String] = typeCheckErrors(body).map(_.message)

  override def spec: Spec[TestEnvironment, Any] = suite("DurableStreamMacroCompileErrorsSpec")(
    test("selector is required with multiple endpoints") {
      val result = errors("""
        import golem.runtime.annotations.*
        import golem.runtime.macros.AgentMacros
        import scala.concurrent.Future
        @agentDefinition(mount = "/") trait A { class Id(); @endpoint(method="POST", path="/a") @endpoint(method="POST", path="/b") @durableStreams() def run(): Future[String] }
        AgentMacros.agentMetadata[A]
      """)
      assertTrue(result.exists(_.contains("selector is ambiguous or missing")))
    },
    test("selector must match an endpoint") {
      val result = errors("""
        import golem.runtime.annotations.*
        import golem.runtime.macros.AgentMacros
        import scala.concurrent.Future
        @agentDefinition(mount = "/") trait A { class Id(); @endpoint(method="POST", path="/a") @durableStreams(endpointMethod="GET", endpointPath="/missing") def run(): Future[String] }
        AgentMacros.agentMetadata[A]
      """)
      assertTrue(result.exists(_.contains("does not match exactly one")))
    },
    test("duplicate slot annotations are rejected") {
      val result = errors("""
        import golem.runtime.annotations.*
        import golem.runtime.macros.AgentMacros
        import scala.concurrent.Future
        @agentDefinition(mount = "/") trait A { class Id(); @endpoint(method="POST", path="/a") @durableStreamSlot(source="input", slot="body") @durableStreamSlot(source="input", slot="body") def run(): Future[String] }
        AgentMacros.agentMetadata[A]
      """)
      assertTrue(result.exists(_.contains("duplicate @durableStreamSlot")))
    },
    test("nonliteral durable stream arguments are rejected") {
      val booleanResult = errors("""
        import golem.runtime.annotations.*
        import golem.runtime.macros.AgentMacros
        import scala.concurrent.Future
        object Policy { def writes: Boolean = false }
        @agentDefinition(mount = "/") trait A { class Id(); @endpoint(method="POST", path="/a") @durableStreams(allowExternalWrites=Policy.writes) def run(): Future[String] }
        AgentMacros.agentMetadata[A]
      """)
      val limitResult = errors("""
        import golem.runtime.annotations.*
        import golem.runtime.macros.AgentMacros
        import scala.concurrent.Future
        object Policy { def readers: Int = 8 }
        @agentDefinition(mount = "/") trait A { class Id(); @endpoint(method="POST", path="/a") @durableStreams(maxConcurrentReadersPerStream=Policy.readers) def run(): Future[String] }
        AgentMacros.agentMetadata[A]
      """)
      val selectorResult = errors("""
        import golem.runtime.annotations.*
        import golem.runtime.macros.AgentMacros
        import scala.concurrent.Future
        object Policy { def method: String = "POST" }
        @agentDefinition(mount = "/") trait A { class Id(); @endpoint(method="POST", path="/a") @durableStreams(endpointMethod=Policy.method, endpointPath="/a") def run(): Future[String] }
        AgentMacros.agentMetadata[A]
      """)
      assertTrue(
        booleanResult.exists(_.contains("must be a boolean literal")),
        limitResult.exists(_.contains("must be an integer literal")),
        selectorResult.exists(_.contains("must be a string literal"))
      )
    }
  )
}
