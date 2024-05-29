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

export class PersistenceLevelGuard implements Disposable {
  constructor(private originalLevel: PersistenceLevel) {}

  [Symbol.dispose]() {
    setOplogPersistenceLevel(this.originalLevel);
  }
}

export function usePersistenceLevel(level: PersistenceLevel): PersistenceLevelGuard {
  const originalLevel = getOplogPersistenceLevel();
  setOplogPersistenceLevel(level);
  return new PersistenceLevelGuard(originalLevel);
}

export function withPersistenceLevel<R>(level: PersistenceLevel, f: () => R): R {
  using _guard = usePersistenceLevel(level);
  return f();
}

export class IdempotenceModeGuard implements Disposable {
  constructor(private original: boolean) {}

  [Symbol.dispose]() {
    setIdempotenceMode(this.original);
  }
}

export function useIdempotenceMode(mode: boolean): IdempotenceModeGuard {
  const original = getIdempotenceMode();
  setIdempotenceMode(mode);
  return new IdempotenceModeGuard(original);
}

export function withIdempotenceMode<R>(mode: boolean, f: () => R): R {
  using _guard = useIdempotenceMode(mode);
  return f();
}


export class RetryPolicyGuard implements Disposable {
  constructor(private original: RetryPolicy) {}

  [Symbol.dispose]() {
    setRetryPolicy(this.original);
  }
}

export function useRetryPolicy(policy: RetryPolicy): RetryPolicyGuard {
  const original = getRetryPolicy();
  setRetryPolicy(policy);
  return new RetryPolicyGuard(original);
}

export function withRetryPolicy<R>(policy: RetryPolicy, f: () => R): R {
  using _guard = useRetryPolicy(policy);
  return f();
}

export class AtomicOperationGuard implements Disposable {
  constructor(private begin: OplogIndex) {}

  [Symbol.dispose]() {
    markEndOperation(this.begin);
  }
}

export function markAtomicOperation(): AtomicOperationGuard {
  const begin = markBeginOperation();
  return new AtomicOperationGuard(begin);
}

export function atomically<T>(f: () => T): T {
  using _guard = markAtomicOperation();
  return f();
}