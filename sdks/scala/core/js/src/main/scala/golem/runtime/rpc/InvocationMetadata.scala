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

import golem.{AgentId, InvocationId}

final case class InvocationMetadata(agentId: AgentId, invocationId: InvocationId)

/** The result of an awaited invocation together with its final identity. */
final case class InvocationResult[+A](metadata: InvocationMetadata, value: A)
