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

import scala.concurrent.Future

final case class InvocationMetadata(agentId: String, idempotencyKey: String)

final case class InvocationResult[+A](metadata: InvocationMetadata, result: A)

final case class InvocationReceipt(metadata: InvocationMetadata)

private[rpc] final case class AsyncInvocation[+A](
  metadata: InvocationMetadata,
  result: Future[A],
  cancellationToken: CancellationToken
)

final case class CancelableAsyncInvocation[+A](
  metadata: InvocationMetadata,
  result: Future[A],
  cancellationToken: CancellationToken
)

final case class CancelableInvocationReceipt(
  metadata: InvocationMetadata,
  cancellationToken: CancellationToken
)
