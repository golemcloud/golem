// Copyright 2024 Golem Cloud
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

import {
  golemCreatePromise as golemCreatePromiseImpl,
  golemAwaitPromise as golemAwaitPromiseImpl,
  golemCompletePromise as golemCompletePromiseImpl,
  golemDeletePromise as golemDeletePromiseImpl,
  getSelfUri as getSelfUriImpl,
  getOplogIndex as getOplogIndexImpl,
  setOplogIndex as setOplogIndexImpl,
  oplogCommit as oplogCommitImpl,
  markBeginOperation as markBeginOperationImpl,
  markEndOperation as markEndOperationImpl,
  getRetryPolicy as getRetryPolicyImpl,
  setRetryPolicy as setRetryPolicyImpl,
  getOplogPersistenceLevel as getOplogPersistenceLevelImpl,
  setOplogPersistenceLevel as setOplogPersistenceLevelImpl,
  getIdempotenceMode as getIdempotenceModeImpl,
  setIdempotenceMode as setIdempotenceModeImpl,
  generateIdempotencyKey as generateIdempotencyKeyImpl,
  updateWorker as updateWorkerImpl,
  GetWorkers,
} from "golem:api/host@0.2.0";

export { GetWorkers };

export function golemCreatePromise() {
  return golemCreatePromiseImpl();
}

export function golemAwaitPromise(promiseId) {
  return golemAwaitPromiseImpl(promiseId);
}

export function golemCompletePromise(promiseId, data) {
  return golemCompletePromiseImpl(promiseId, data);
}

export function golemDeletePromise(promiseId) {
  return golemDeletePromiseImpl(promiseId);
}

export function getSelfUri(functionName) {
  return getSelfUriImpl(functionName);
}

export function getOplogIndex() {
  return getOplogIndexImpl();
}

export function setOplogIndex(oplogIdx) {
  return setOplogIndexImpl(oplogIdx);
}

export function oplogCommit(replicas) {
  return oplogCommitImpl(replicas);
}

export function markBeginOperation() {
  return markBeginOperationImpl();
}

export function markEndOperation(begin) {
  return markEndOperationImpl(begin);
}

export function getRetryPolicy() {
  return getRetryPolicyImpl();
}

export function setRetryPolicy(newRetryPolicy) {
  return setRetryPolicyImpl(newRetryPolicy);
}

export function getOplogPersistenceLevel() {
  return getOplogPersistenceLevelImpl();
}

export function setOplogPersistenceLevel(newPersistenceLevel) {
  return setOplogPersistenceLevelImpl(newPersistenceLevel);
}

export function getIdempotenceMode() {
  return getIdempotenceModeImpl();
}

export function setIdempotenceMode(idempotent) {
  return setIdempotenceModeImpl(idempotent);
}

export function generateIdempotencyKey() {
  return generateIdempotencyKeyImpl();
}

export function updateWorker(workerId, targetVersion, mode) {
  return updateWorkerImpl(workerId, targetVersion, mode);
}
