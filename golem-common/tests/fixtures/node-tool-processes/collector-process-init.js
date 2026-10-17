// Feasibility adapter only. The probe prepares typed wire inputs, not CLI argv.
import { setProcessBackend } from 'wasm-rquickjs:child-process';
import { ToolRpc, createCollectedOutput } from 'golem:tool/host@0.1.0';

setProcessBackend({
    spawnSync(command, args, options) {
        if (command !== 'streaming') return undefined;
        const request = JSON.parse(args[0]);
        const [target, stdout] = createCollectedOutput(BigInt(options.maxBuffer));
        let stderr;
        try {
            stderr = request.stderr ? createCollectedOutput(BigInt(options.maxBuffer)) : undefined;
        } catch (error) {
            target[Symbol.dispose]?.();
            stdout[Symbol.dispose]?.();
            throw error;
        }
        const rpc = new ToolRpc(command);
        const future = rpc.asyncInvokeAndAwait(request.path, request.input, undefined, target, stderr?.[0]);
        const collected = future.collect(stdout, stderr?.[1]);
        future[Symbol.dispose]?.();
        return {
            pid: 1,
            status: collected.result.tag === 'ok' ? 0 : null,
            signal: null,
            stdout: collected.stdout?.bytes,
            stderr: collected.stderr?.bytes,
            // Preserve the raw terminal for transport assertions. Production
            // must map tool result schemas and failures to Node exit policy.
            collected,
        };
    },
});
