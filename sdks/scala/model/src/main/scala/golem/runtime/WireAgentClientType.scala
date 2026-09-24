/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 */

package golem.runtime

import golem.schema.wire.ConcreteCodec

final case class WireAgentClientType[Trait, Ctor](
  metadata: WireAgentMetadata,
  ctorCodec: ConcreteCodec[Ctor],
  methods: List[WireClientMethod[Trait]]
)

trait WireClientMethod[Trait] {
  type Input
  type Output
  def name: String
  def input: ConcreteCodec[Input]
  def output: Option[ConcreteCodec[Output]]
}

object WireClientMethod {
  def apply[Trait, In, Out](
    methodName: String,
    inputCodec: ConcreteCodec[In],
    outputCodec: Option[ConcreteCodec[Out]]
  ): WireClientMethod[Trait] { type Input = In; type Output = Out } =
    new WireClientMethod[Trait] {
      type Input  = In
      type Output = Out
      val name                               = methodName
      val input: ConcreteCodec[Input]        = inputCodec
      val output: Option[ConcreteCodec[Out]] = outputCodec
    }
}
