declare module 'golem:video-generation/lip-sync@1.0.0' {
  import * as golemVideoGeneration100Types from 'golem:video-generation/types@1.0.0';
  export function generateLipSync(video: LipSyncVideo, audio: AudioSource): Result<string, VideoError>;
  export function listVoices(language: string | undefined): Result<VoiceInfo[], VideoError>;
  export type BaseVideo = golemVideoGeneration100Types.BaseVideo;
  export type AudioSource = golemVideoGeneration100Types.AudioSource;
  export type VideoError = golemVideoGeneration100Types.VideoError;
  export type VoiceInfo = golemVideoGeneration100Types.VoiceInfo;
  export type LipSyncVideo = golemVideoGeneration100Types.LipSyncVideo;
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
