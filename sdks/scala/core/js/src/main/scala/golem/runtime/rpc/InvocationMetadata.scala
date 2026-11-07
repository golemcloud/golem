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

import golem.InvocationId

import scala.concurrent.Future

final case class InvocationMetadata(agentId: String, invocationId: InvocationId)

final case class InvocationResult[+A](metadata: InvocationMetadata, value: A)

final case class InvocationReceipt(metadata: InvocationMetadata)

private[rpc] final case class AsyncInvocation[+A](
  metadata: Option[InvocationMetadata],
  result: Future[A],
  cancellationToken: CancellationToken
)

/**
 * A started asynchronous invocation. `metadata` is `None` when the host
 * rejected the call before it started; `result` then fails with the rejection
 * error.
 */
final case class CancelableAsyncInvocation[+A](
  metadata: Option[InvocationMetadata],
  result: Future[A],
  cancellationToken: CancellationToken
)

final case class CancelableInvocationReceipt(
  metadata: InvocationMetadata,
  cancellationToken: CancellationToken
)
