declare module 'golem:video-generation/advanced@1.0.0' {
  import * as golemVideoGeneration100Types from 'golem:video-generation/types@1.0.0';
  /**
   * @throws VideoError
   */
  export function extendVideo(options: ExtendVideoOptions): string;
  /**
   * @throws VideoError
   */
  export function upscaleVideo(input: BaseVideo): string;
  /**
   * @throws VideoError
   */
  export function generateVideoEffects(options: GenerateVideoEffectsOptions): string;
  /**
   * @throws VideoError
   */
  export function multiImageGeneration(options: MultImageGenerationOptions): string;
  export type VideoError = golemVideoGeneration100Types.VideoError;
  export type Kv = golemVideoGeneration100Types.Kv;
  export type BaseVideo = golemVideoGeneration100Types.BaseVideo;
  export type GenerationConfig = golemVideoGeneration100Types.GenerationConfig;
  export type InputImage = golemVideoGeneration100Types.InputImage;
  export type EffectType = golemVideoGeneration100Types.EffectType;
  export type ExtendVideoOptions = {
    videoId: string;
    prompt?: string;
    negativePrompt?: string;
    cfgScale?: number;
    providerOptions?: Kv[];
  };
  export type GenerateVideoEffectsOptions = {
    input: InputImage;
    effect: EffectType;
    model?: string;
    duration?: number;
    mode?: string;
  };
  export type MultImageGenerationOptions = {
    inputImages: InputImage[];
    prompt?: string;
    config: GenerationConfig;
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
