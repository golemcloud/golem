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

import { getIdempotenceMode, getOplogPersistenceLevel, getRetryPolicy, markBeginOperation, markEndOperation, OplogIndex, PersistenceLevel, RetryPolicy, setIdempotenceMode, setOplogPersistenceLevel, setRetryPolicy } from "./bindgen/bindgen";

export class PersistenceLevelGuard {
  constructor(private originalLevel: PersistenceLevel) {}

  drop() {
    console.log("Dropping PersistenceLevelGuard!")
    setOplogPersistenceLevel(this.originalLevel);
  }
}

export function usePersistenceLevel(level: PersistenceLevel) {
  const originalLevel = getOplogPersistenceLevel();
  setOplogPersistenceLevel(level);
  return new PersistenceLevelGuard(originalLevel);
}

export function withPersistenceLevel<R>(level: PersistenceLevel, f: () => R): R {
  const guard = usePersistenceLevel(level);
  return executeWithDrop([guard], f);
}

export class IdempotenceModeGuard {
  constructor(private original: boolean) {}

  drop() {
    console.log("Dropping RetryPolicyGuard!")
    setIdempotenceMode(this.original);
  }
}

export function useIdempotenceMode(mode: boolean): IdempotenceModeGuard {
  const original = getIdempotenceMode();
  setIdempotenceMode(mode);
  return new IdempotenceModeGuard(original);
}

export function withIdempotenceMode<R>(mode: boolean, f: () => R): R {
  const guard = useIdempotenceMode(mode);
  return executeWithDrop([guard], f);
}


export class RetryPolicyGuard {
  constructor(private original: RetryPolicy) {}

  drop() {
    console.log("Dropping RetryPolicyGuard!")
    setRetryPolicy(this.original);
  }
}

export function useRetryPolicy(policy: RetryPolicy): RetryPolicyGuard {
  const original = getRetryPolicy();
  setRetryPolicy(policy);
  return new RetryPolicyGuard(original);
}

export function withRetryPolicy<R>(policy: RetryPolicy, f: () => R): R {
  const guard = useRetryPolicy(policy);
  return executeWithDrop([guard], f);
}

export class AtomicOperationGuard {
  constructor(private begin: OplogIndex) {}

  drop() {
    console.log("Dropping AtomicOperationGuard!")
    markEndOperation(this.begin);
  }
}

export function markAtomicOperation(): AtomicOperationGuard {
  const begin = markBeginOperation();
  return new AtomicOperationGuard(begin);
}

export function atomically<T>(f: () => T): T {
  let guard = markAtomicOperation();
  return executeWithDrop([guard], f);
}


export function executeWithDrop<Resource extends {drop: () => void}, R>(
  resources: [Resource],
  fn: () => R
): R {
  try {
    return fn();
  } finally {
    dropAll(resources);
  }
}

function dropAll<Resource extends {drop: () => void}>(resources: [Resource]) {
  console.log("dropAll!")
  const errors = [];
  for (const resource of resources) {
    try {
      resource.drop();
    } catch (e) {
      errors.push(e);
    }
  }
  if (errors.length > 0) {
    throw new DropError(errors);
  }
}

class DropError extends Error {
  constructor(public errors: Error[]) {
    super("Error dropping resources", { cause: errors[0] });
  }
}