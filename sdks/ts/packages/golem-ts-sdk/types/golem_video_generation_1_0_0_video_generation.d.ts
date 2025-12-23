declare module 'golem:video-generation/video-generation@1.0.0' {
  import * as golemVideoGeneration100Types from 'golem:video-generation/types@1.0.0';
  /**
   * @throws VideoError
   */
  export function generate(input: MediaInput, config: GenerationConfig): string;
  /**
   * @throws VideoError
   */
  export function poll(jobId: string): VideoResult;
  /**
   * @throws VideoError
   */
  export function cancel(jobId: string): string;
  export type MediaInput = golemVideoGeneration100Types.MediaInput;
  export type GenerationConfig = golemVideoGeneration100Types.GenerationConfig;
  export type VideoResult = golemVideoGeneration100Types.VideoResult;
  export type VideoError = golemVideoGeneration100Types.VideoError;
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
