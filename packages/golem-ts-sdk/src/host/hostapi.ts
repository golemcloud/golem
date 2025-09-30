// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

import { PromiseId, createPromise, getPromise } from 'golem:api/host@1.1.7';

// reexport golem host api
export {
  getOplogIndex,
  setOplogIndex,
  oplogCommit,
  markBeginOperation,
  markEndOperation,
  getRetryPolicy,
  setRetryPolicy,
  getOplogPersistenceLevel,
  setOplogPersistenceLevel,
  getIdempotenceMode,
  setIdempotenceMode,
  generateIdempotencyKey,
  updateWorker,
  getSelfMetadata,
  getWorkerMetadata,
  forkWorker,
  revertWorker,
  resolveComponentId,
  resolveWorkerId,
  resolveWorkerIdStrict,
  fork,
  GetWorkers,
  Duration,
  ComponentId,
  Uuid,
  ValueAndType,
  WorkerId,
  Pollable,
  OplogIndex,
  PromiseId,
  ComponentVersion,
  AccountId,
  ProjectId,
  RetryPolicy,
  PersistenceLevel,
  UpdateMode,
  FilterComparator,
  StringFilterComparator,
  WorkerStatus,
  WorkerNameFilter,
  WorkerStatusFilter,
  WorkerVersionFilter,
  WorkerCreatedAtFilter,
  WorkerEnvFilter,
  WorkerWasiConfigVarsFilter,
  WorkerPropertyFilter,
  WorkerAllFilter,
  WorkerAnyFilter,
  WorkerMetadata,
  RevertWorkerTarget,
  ForkResult
} from 'golem:api/host@1.1.7';

export function createGolemPromise(): PromiseId {
  return createPromise()
}

export async function awaitGolemPromise(promiseId: PromiseId): Promise<Uint8Array> {
  const promise = getPromise(promiseId);
  await promise.subscribe().promise();
  return promise.get()!
}
