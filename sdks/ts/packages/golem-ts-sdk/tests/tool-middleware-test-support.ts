import type { InvocationResult } from 'golem:tool/common@0.1.0';
import type { UnderlyingTool } from 'golem:tool/underlying@0.1.0';
import type { ByteStreamItem } from 'golem:tool/streams@0.1.0';
import { ToolInvokeError } from '../src/tool';

export interface LegacyRawUnderlyingTool {
  invoke(
    commandPath: string[],
    input: unknown,
    stdin: AsyncIterable<number> | undefined,
  ): Promise<InvocationResult>;
}

export function adaptLegacyRawUnderlying(
  legacy: LegacyRawUnderlyingTool,
): Pick<UnderlyingTool, 'invoke'> {
  return {
    async invoke(commandPath, input, stdin) {
      const carrier = Promise.resolve(
        legacy.invoke(commandPath, input, stdin === undefined ? undefined : decodeStdin(stdin)),
      );
      const resolved = await carrier;
      if (resolved === null || typeof resolved !== 'object' || Array.isArray(resolved)) {
        throw new ToolInvokeError({
          tag: 'invalid-result',
          val: 'tool invocation result must be an object',
        });
      }
      const stdout = resolved.stdout;
      return [
        {
          get: async () => (await carrier).result,
          cancel: () => undefined,
        },
        stdout === undefined ? undefined : encodeStdout(stdout),
      ] as never;
    },
  };
}

function encodeStdout(stdout: AsyncIterable<number>): AsyncIterable<ByteStreamItem> {
  return {
    [Symbol.asyncIterator]() {
      const iterator = stdout[Symbol.asyncIterator]();
      return {
        async next() {
          const next = await iterator.next();
          return next.done
            ? { done: true, value: undefined }
            : { done: false, value: { tag: 'ok', val: Uint8Array.of(next.value) } as const };
        },
        async return() {
          await iterator.return?.();
          return { done: true, value: undefined };
        },
      };
    },
  };
}

function decodeStdin(stdin: AsyncIterable<ByteStreamItem>): AsyncIterable<number> {
  const iterator = stdin[Symbol.asyncIterator]();
  let chunk: Uint8Array | undefined;
  let offset = 0;
  return {
    [Symbol.asyncIterator]() {
      return {
        async next(): Promise<IteratorResult<number>> {
          while (chunk === undefined || offset === chunk.byteLength) {
            const next = await iterator.next();
            if (next.done) return { done: true, value: undefined };
            if (next.value.tag === 'err') throw next.value.val;
            chunk = next.value.val;
            offset = 0;
          }
          return { done: false, value: chunk[offset++] };
        },
        async return() {
          await iterator.return?.();
          return { done: true, value: undefined };
        },
      };
    },
  };
}
