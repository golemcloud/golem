/// <reference path="generated/wit-generated.d.ts" />

import type * as World from 'rpc:caller/caller';

import { Counter } from "rpc:counters-client/counters-client";
import { getEnvironment } from "wasi:cli/environment@0.2.3";
import { parseUuid } from "golem:rpc/types@0.2.2";

let globalCounter: Counter | undefined = undefined;

function getEnvironmentValue(key: string): string {
  const env = getEnvironment();
  const value = env.find(([name, _]) => name === key)?.[1];

  if (value === undefined) {
    throw new Error(`${key} not set in env`);
  }

  return value;
}

function createGlobalCounter(workerName: string): Counter {
  const counterId = getEnvironmentValue("COUNTERS_COMPONENT_ID");
  console.log(`counterId = ${counterId}`);
  const uuid = parseUuid(counterId);
  const componentId = { uuid };
  const workerId = {
    componentId,
    workerName,
  };
  console.log(`Creating counter resource on ${workerId}`);
  const counter = Counter.custom(workerId, workerName);
  console.log(`Created counter resource on ${workerId}: ${counter}`);
  globalCounter = counter;
  return counter;
}

function test1(): bigint {
  const counter = globalCounter ?? createGlobalCounter("counters_test2");
  counter.blockingIncBy(BigInt(1));
  const value = counter.blockingGetValue();
  return value;
}

function test2(): bigint {
  const counter = globalCounter ?? createGlobalCounter("counters_test3");
  counter.blockingIncBy(BigInt(1));
  const value = counter.blockingGetValue();
  return value;
}

function test3(): [string[], [string, string][]] {
  const counter = globalCounter ?? createGlobalCounter("counters_test4");
  const args = counter.blockingGetArgs();
  const env = counter.blockingGetEnv();
  return [args, env];
}

function test5(): BigUint64Array {
  return new BigUint64Array(0);
}

export const callerInlineFunctions: typeof World.callerInlineFunctions = {
  test1,
  test2,
  test3,
};
