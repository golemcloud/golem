// ../lib/src/index.js
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
  GetWorkers
} from "golem:api/host@0.2.0";
function golemCreatePromise() {
  return golemCreatePromiseImpl();
}

// index.js
var api = {
  createPromise() {
    let promiseId = golemCreatePromise();
    console.log("Created Promise", promiseId);
    return promiseId;
  }
};
export {
  api
};
