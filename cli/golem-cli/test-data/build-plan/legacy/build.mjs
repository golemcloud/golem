import { copyFileSync } from 'node:fs';

copyFileSync('seed.wasm', 'component.wasm');
copyFileSync('component.wasm', 'staged.wasm');
