declare module 'golem:video-generation/advanced@1.0.0' {
  import * as golemVideoGeneration100Types from 'golem:video-generation/types@1.0.0';
  export function extendVideo(videoId: string, prompt: string | undefined, negativePrompt: string | undefined, cfgScale: number | undefined, providerOptions: Kv[] | undefined): Result<string, VideoError>;
  export function upscaleVideo(input: BaseVideo): Result<string, VideoError>;
  export function generateVideoEffects(input: InputImage, effect: EffectType, model: string | undefined, duration: number | undefined, mode: string | undefined): Result<string, VideoError>;
  export function multiImageGeneration(inputImages: InputImage[], prompt: string | undefined, config: GenerationConfig): Result<string, VideoError>;
  export type VideoError = golemVideoGeneration100Types.VideoError;
  export type Kv = golemVideoGeneration100Types.Kv;
  export type BaseVideo = golemVideoGeneration100Types.BaseVideo;
  export type GenerationConfig = golemVideoGeneration100Types.GenerationConfig;
  export type InputImage = golemVideoGeneration100Types.InputImage;
  export type EffectType = golemVideoGeneration100Types.EffectType;
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
