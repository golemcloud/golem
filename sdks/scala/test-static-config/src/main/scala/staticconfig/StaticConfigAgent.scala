// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
package staticconfig

import golem.BaseAgent
import golem.config.{AgentConfig, Config}
import golem.runtime.annotations.{agentDefinition, agentImplementation}
import zio.blocks.schema.{Modifier, Schema}
import scala.concurrent.Future

final case class Settings(label: String, count: golem.UInt) derives Schema
final case class Node(display_name: String, next: Option[Node]) derives Schema
final case class Values(
  @Modifier.config("golem.config.default", "{\"label\":\"code-default\",\"count\":31}") settings: Settings,
  @Modifier.config("golem.config.default", "[null,7,\"retained\"]") slots: (Option[Int], Option[Int], String),
  @Modifier.config("golem.config.default", "4000") max_chars: Int,
  @Modifier.config("golem.config.default", "null") recursive: Option[Node],
  @Modifier.config("golem.config.default", "true") enabled: Boolean
) derives Schema

@agentDefinition()
trait StaticConfigAgent extends BaseAgent with AgentConfig[Values] {
  class Id(val name: String)
  def read(): Future[String]
}

@agentImplementation()
final class StaticConfigAgentImpl(name: String, config: Config[Values]) extends StaticConfigAgent {
  private val captured = render()

  private def render(): String =
    s"$name|${config.settings.label}|${config.settings.count}|${config.slots}|${config.max_chars}|${config.recursive}|${config.enabled}"

  def read(): Future[String] = Future.successful(s"$captured;${render()}")
}
