export const templateMatrix = [
  {
    role: 'default',
    world: 'agent-guest',
    wrapperDirectory: 'agent-template',
    sdkModuleName: '@golemcloud/golem-ts-sdk',
    sdkEntry: 'dist/index.mjs',
    cargoArtifact: 'agent_guest.wasm',
    wasmFile: 'agent_guest.wasm',
    declarationFile: 'exports.d.ts',
  },
];
