import { setProcessBackend } from 'wasm-rquickjs:child-process';
import { execute } from 'experiment:node-tools/host';

// A real host boundary, not a substitute for Golem command parsing/transport.
// No host calls run during registration or Wizer initialization.
let calls = 0;
setProcessBackend({
    spawnSync(command, args) {
        if (command !== 'answer') return undefined;
        if (++calls !== 1) throw new Error('Backend state was reused across fresh invocations');
        const result = execute(Number(args[0]));
        if (result && typeof result.then === 'function') throw new Error('Expected synchronous import');
        return { pid: 1, status: 0, signal: null, stdout: new TextEncoder().encode(String(result)), stderr: new Uint8Array() };
    },
});
