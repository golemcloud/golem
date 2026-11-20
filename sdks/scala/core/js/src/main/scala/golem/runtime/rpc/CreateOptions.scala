/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 */

package golem.runtime.rpc

import golem.config.ConfigOverride

/**
 * Options of a client `create`.
 *
 * @param keyed
 *   Creates the keyed agent of the constructor parameters, and fails with
 *   [[AgentRpcError.Conflict]] when it already exists. Otherwise the new agent
 *   has no key and a new ID.
 * @param config
 *   Overrides of the agent type's local configuration values
 * @param env
 *   Environment variables added to the ones the manifest defines. The caller's
 *   own environment is not copied.
 */
final case class CreateOptions(
  keyed: Boolean = false,
  config: List[ConfigOverride] = Nil,
  env: Map[String, String] = Map.empty
)
