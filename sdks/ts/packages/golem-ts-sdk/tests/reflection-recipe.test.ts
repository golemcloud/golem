import { describe, expect, it } from 'vitest';
import {
  getReflectedAgentType,
  isRemoteCallError,
} from '@golemcloud/golem-ts-sdk';

async function callDynamically() {
  const type = getReflectedAgentType('SearchAgent');
  const method = type?.method('search');
  if (!type || type.mode !== 'durable' || !method) {
    throw new Error('SearchAgent.search is unavailable');
  }

  const input = method.input.packJson({ query: 'golem', cursor: null });
  const inputCheck = method.input.validateValue(input);
  if (!inputCheck.success) throw new Error(JSON.stringify(inputCheck.issues));

  const id = type.agentId({ tenant: 'docs' });
  try {
    const result = await id.dynamicClient().method(method.name).invokeValue(input);
    if (!method.output || result.value === undefined) {
      throw new Error('search returned an unexpected unit result');
    }
    const outputCheck = method.output.validateValue(result.value);
    if (!outputCheck.success) throw new Error(JSON.stringify(outputCheck.issues));
    return method.output.unpackJson(result.value);
  } catch (error) {
    if (isRemoteCallError(error)) {
      console.error('dynamic search failed', error.cause);
    }
    throw error;
  }
}

describe('discovery-to-dynamic guide', () => {
  it('keeps its recipe typechecked without contacting the host', () => {
    expect(callDynamically).toBeTypeOf('function');
  });
});
