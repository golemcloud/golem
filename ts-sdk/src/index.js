import {
  golemCreatePromise,
  golemAwaitPromise,
  golemCompletePromise,
  golemDeletePromise,
  getSelfUri,
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
  GetWorkers,
} from "golem:api/host@0.2.0";

export module GolemApiHost {
  export { GetWorkers };

  export function golemCreatePromise() {
    return golemCreatePromise();
  }

  export function golemAwaitPromise(promiseId) {
    return golemAwaitPromise(promiseId);
  }

  export function golemCompletePromise(promiseId, data) {
    return golemCompletePromise(promiseId, data);
  }

  export function golemDeletePromise(promiseId) {
    return golemDeletePromise(promiseId);
  }

  export function getSelfUri(functionName) {
    return getSelfUri(functionName);
  }

  export function getOplogIndex() {
    return getOplogIndex();
  }

  export function setOplogIndex(oplogIdx) {
    return setOplogIndex(oplogIdx);
  }

  export function oplogCommit(replicas) {
    return oplogCommit(replicas);
  }

  export function markBeginOperation() {
    return markBeginOperation();
  }

  export function markEndOperation(begin) {
    return markEndOperation(begin);
  }

  export function getRetryPolicy() {
    return getRetryPolicy();
  }

  export function setRetryPolicy(newRetryPolicy) {
    return setRetryPolicy(newRetryPolicy);
  }

  export function getOplogPersistenceLevel() {
    return getOplogPersistenceLevel();
  }

  export function setOplogPersistenceLevel(newPersistenceLevel) {
    return setOplogPersistenceLevel(newPersistenceLevel);
  }

  export function getIdempotenceMode() {
    return getIdempotenceMode();
  }

  export function setIdempotenceMode(idempotent) {
    return setIdempotenceMode(idempotent);
  }

  export function generateIdempotencyKey() {
    return generateIdempotencyKey();
  }

  export function updateWorker(workerId, targetVersion, mode) {
    return updateWorker(workerId, targetVersion, mode);
  }
}