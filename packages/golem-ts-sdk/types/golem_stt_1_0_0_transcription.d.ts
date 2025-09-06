declare module 'golem:stt/transcription@1.0.0' {
  import * as golemStt100Types from 'golem:stt/types@1.0.0';
  export function transcribe(request: TranscriptionRequest): Result<TranscriptionResult, SttError>;
  export function transcribeMany(requests: TranscriptionRequest[]): Result<MultiTranscriptionResult, SttError>;
  export type AudioConfig = golemStt100Types.AudioConfig;
  export type TranscriptionResult = golemStt100Types.TranscriptionResult;
  export type SttError = golemStt100Types.SttError;
  export type LanguageCode = golemStt100Types.LanguageCode;
  export type Phrase = {
    value: string;
    boost: number | undefined;
  };
  export type Vocabulary = {
    phrases: Phrase[];
  };
  export type DiarizationOptions = {
    enabled: boolean;
    minSpeakerCount: number | undefined;
    maxSpeakerCount: number | undefined;
  };
  export type TranscribeOptions = {
    language: LanguageCode | undefined;
    model: string | undefined;
    profanityFilter: boolean | undefined;
    vocabulary: Vocabulary | undefined;
    diarization: DiarizationOptions | undefined;
    enableMultiChannel: boolean | undefined;
  };
  export type TranscriptionRequest = {
    requestId: string;
    audio: Uint8Array;
    config: AudioConfig;
    options: TranscribeOptions | undefined;
  };
  export type FailedTranscription = {
    requestId: string;
    error: SttError;
  };
  export type MultiTranscriptionResult = {
    successes: TranscriptionResult[];
    failures: FailedTranscription[];
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
