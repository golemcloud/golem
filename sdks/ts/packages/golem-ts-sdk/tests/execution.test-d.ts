import { runJavaScript, startJavaScript } from 'wasm-rquickjs:execution';

const entry = runJavaScript<{ answer: number }>({ entry: './main.ts', timeoutMs: 1_000 });
const source = runJavaScript<number>({
  source: 'return 42 as number;',
  language: 'typescript',
  overflow: 'truncate',
});
const job = startJavaScript({ source: 'console.log("hello");' });

entry.then((result) => result.value.answer);
source.then((result) => result.overflowed);
job.stdout.on('data', () => undefined);
job.result.then((result) => result.value);
job.cancel();

// @ts-expect-error entry and source are mutually exclusive
runJavaScript({ entry: './main.ts', source: 'return 1;' });
// @ts-expect-error language only applies to inline source
runJavaScript({ entry: './main.ts', language: 'typescript' });
// @ts-expect-error a program is required
runJavaScript({ timeoutMs: 1_000 });
