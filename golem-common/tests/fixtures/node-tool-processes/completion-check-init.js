import process from 'node:process';
import { setProcessBackend } from 'wasm-rquickjs:child-process';
import * as host from 'golem:tool/host@0.1.0';
import { createToolProcessBackend } from 'golem-tool-process-backend';

// This guest facade uses the production host calls. It checks their recorded
// classification without changing the process adapter or adding a host hook.
function CheckedToolRpc(name) {
    const rpc = new host.ToolRpc(name);
    return {
        asyncInvokeAndAwait(path, input, stdin, stdout, stderr) {
            const rejected = rpc.asyncInvokeAndAwait(
                ['command-that-does-not-exist'], input, undefined, undefined, undefined,
            );
            try {
                const result = rejected.collect(undefined, undefined);
                if (result.bodyExecuted !== false || result.result.tag !== 'err') {
                    throw new Error('Rejected command did not retain its skipped classification');
                }
            } finally {
                rejected[Symbol.dispose]();
            }
            const future = rpc.asyncInvokeAndAwait(path, input, stdin, stdout, stderr);
            return {
                collect(out, err) {
                    const result = future.collect(out, err);
                    if (result.bodyExecuted !== true || result.result.tag !== 'ok') {
                        throw new Error('Filesystem command did not retain its executed classification');
                    }
                    return result;
                },
                cancel() { future.cancel(); },
                [Symbol.dispose]() { future[Symbol.dispose](); },
            };
        },
        [Symbol.dispose]() { rpc[Symbol.dispose](); },
    };
}

setProcessBackend(createToolProcessBackend({ ...host, ToolRpc: CheckedToolRpc }, process));
