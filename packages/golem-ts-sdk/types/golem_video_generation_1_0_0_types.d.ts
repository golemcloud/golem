declare module 'golem:video-generation/types@1.0.0' {
  export type VideoError = {
    tag: 'invalid-input'
    val: string
  } |
  {
    tag: 'unsupported-feature'
    val: string
  } |
  {
    tag: 'quota-exceeded'
  } |
  {
    tag: 'generation-failed'
    val: string
  } |
  {
    tag: 'cancelled'
  } |
  {
    tag: 'internal-error'
    val: string
  };
  export type ImageRole = "first" | "last";
  export type RawBytes = {
    bytes: Uint8Array;
    mimeType: string;
  };
  export type MediaData = {
    tag: 'url'
    val: string
  } |
  {
    tag: 'bytes'
    val: RawBytes
  };
  export type InputImage = {
    data: MediaData;
  };
  export type Reference = {
    data: InputImage;
    prompt: string | undefined;
    role: ImageRole | undefined;
  };
  export type BaseVideo = {
    data: MediaData;
  };
  export type MediaInput = {
    tag: 'text'
    val: string
  } |
  {
    tag: 'image'
    val: Reference
  } |
  {
    tag: 'video'
    val: BaseVideo
  };
  export type Narration = {
    data: MediaData;
  };
  export type StaticMask = {
    mask: InputImage;
  };
  export type Position = {
    x: number;
    y: number;
  };
  export type DynamicMask = {
    mask: InputImage;
    trajectories: Position[];
  };
  export type CameraConfig = {
    horizontal: number;
    vertical: number;
    pan: number;
    tilt: number;
    zoom: number;
    roll: number;
  };
  export type CameraMovement = {
    tag: 'simple'
    val: CameraConfig
  } |
  {
    tag: 'down-back'
  } |
  {
    tag: 'forward-up'
  } |
  {
    tag: 'right-turn-forward'
  } |
  {
    tag: 'left-turn-forward'
  };
  export type AspectRatio = "square" | "portrait" | "landscape" | "cinema";
  export type Resolution = "sd" | "hd" | "fhd" | "uhd";
  export type Kv = {
    key: string;
    value: string;
  };
  export type GenerationConfig = {
    negativePrompt: string | undefined;
    seed: bigint | undefined;
    scheduler: string | undefined;
    guidanceScale: number | undefined;
    aspectRatio: AspectRatio | undefined;
    durationSeconds: number | undefined;
    resolution: Resolution | undefined;
    model: string | undefined;
    enableAudio: boolean | undefined;
    enhancePrompt: boolean | undefined;
    providerOptions: Kv[] | undefined;
    lastframe: InputImage | undefined;
    staticMask: StaticMask | undefined;
    dynamicMask: DynamicMask | undefined;
    cameraControl: CameraMovement | undefined;
  };
  export type Video = {
    uri: string | undefined;
    base64Bytes: Uint8Array | undefined;
    mimeType: string;
    width: number | undefined;
    height: number | undefined;
    fps: number | undefined;
    durationSeconds: number | undefined;
    generationId: string | undefined;
  };
  export type JobStatus = {
    tag: 'pending'
  } |
  {
    tag: 'running'
  } |
  {
    tag: 'succeeded'
  } |
  {
    tag: 'failed'
    val: string
  };
  export type VideoResult = {
    status: JobStatus;
    videos: Video[] | undefined;
  };
  export type VoiceLanguage = "en" | "zh";
  export type TextToSpeech = {
    text: string;
    voiceId: string;
    language: VoiceLanguage;
    speed: number;
  };
  export type AudioSource = {
    tag: 'from-text'
    val: TextToSpeech
  } |
  {
    tag: 'from-audio'
    val: Narration
  };
  export type VoiceInfo = {
    voiceId: string;
    name: string;
    language: VoiceLanguage;
    previewUrl: string | undefined;
  };
  export type SingleImageEffects = "bloombloom" | "dizzydizzy" | "fuzzyfuzzy" | "squish" | "expansion" | "anime-figure" | "rocketrocket";
  export type DualImageEffects = "hug" | "kiss" | "heart-gesture";
  export type DualEffect = {
    effect: DualImageEffects;
    secondImage: InputImage;
  };
  export type EffectType = {
    tag: 'single'
    val: SingleImageEffects
  } |
  {
    tag: 'dual'
    val: DualEffect
  };
  export type LipSyncVideo = {
    tag: 'video'
    val: BaseVideo
  } |
  {
    tag: 'video-id'
    val: string
  };
}
