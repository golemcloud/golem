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
