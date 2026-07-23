declare module 'wasi:cli/stdout@0.2.3' {
  import * as wasiIo023Streams from 'wasi:io/streams@0.2.3';
  export function getStdout(): OutputStream;
  export type OutputStream = wasiIo023Streams.OutputStream;
}
