declare module 'golem:video-generation/video-generation@1.0.0' {
  import * as golemVideoGeneration100Types from 'golem:video-generation/types@1.0.0';
  export function generate(input: MediaInput, config: GenerationConfig): Result<string, VideoError>;
  export function poll(jobId: string): Result<VideoResult, VideoError>;
  export function cancel(jobId: string): Result<string, VideoError>;
  export type MediaInput = golemVideoGeneration100Types.MediaInput;
  export type GenerationConfig = golemVideoGeneration100Types.GenerationConfig;
  export type VideoResult = golemVideoGeneration100Types.VideoResult;
  export type VideoError = golemVideoGeneration100Types.VideoError;
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
