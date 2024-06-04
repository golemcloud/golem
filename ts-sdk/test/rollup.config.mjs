import typescript from '@rollup/plugin-typescript';

export default [{
  input: 'src/index.ts',
  output: {
    file: 'out/build.js',
    format: 'esm', 
  },
  external: ['golem:api/host@0.2.0'],
  plugins: [typescript()]
}];