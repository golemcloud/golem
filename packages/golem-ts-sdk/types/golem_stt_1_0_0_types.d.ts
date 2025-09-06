declare module 'golem:stt/types@1.0.0' {
  export type SttError = {
    tag: 'invalid-audio'
    val: string
  } |
  {
    tag: 'unsupported-format'
    val: string
  } |
  {
    tag: 'unsupported-language'
    val: string
  } |
  {
    tag: 'transcription-failed'
    val: string
  } |
  {
    tag: 'unauthorized'
    val: string
  } |
  {
    tag: 'access-denied'
    val: string
  } |
  {
    tag: 'rate-limited'
    val: string
  } |
  {
    tag: 'insufficient-credits'
  } |
  {
    tag: 'unsupported-operation'
    val: string
  } |
  {
    tag: 'service-unavailable'
    val: string
  } |
  {
    tag: 'network-error'
    val: string
  } |
  {
    tag: 'internal-error'
    val: string
  };
  export type LanguageCode = string;
  export type AudioFormat = "wav" | "mp3" | "flac" | "ogg" | "aac" | "pcm";
  export type AudioConfig = {
    format: AudioFormat;
    sampleRate: number | undefined;
    channels: number | undefined;
  };
  export type TimingInfo = {
    startTimeSeconds: number;
    endTimeSeconds: number;
  };
  export type WordSegment = {
    text: string;
    timingInfo: TimingInfo | undefined;
    confidence: number | undefined;
    speakerId: string | undefined;
  };
  export type TranscriptionMetadata = {
    durationSeconds: number;
    audioSizeBytes: number;
    requestId: string;
    model: string | undefined;
    language: LanguageCode;
  };
  export type TranscriptionSegment = {
    transcript: string;
    timingInfo: TimingInfo | undefined;
    speakerId: string | undefined;
    words: WordSegment[];
  };
  export type TranscriptionChannel = {
    id: string;
    transcript: string;
    segments: TranscriptionSegment[];
  };
  export type TranscriptionResult = {
    transcriptMetadata: TranscriptionMetadata;
    channels: TranscriptionChannel[];
  };
}
