declare module 'golem:stt/transcription@1.0.0' {
  import * as golemStt100Types from 'golem:stt/types@1.0.0';
  /**
   * @throws SttError
   */
  export function transcribe(request: TranscriptionRequest): TranscriptionResult;
  /**
   * @throws SttError
   */
  export function transcribeMany(requests: TranscriptionRequest[]): MultiTranscriptionResult;
  export type AudioConfig = golemStt100Types.AudioConfig;
  export type TranscriptionResult = golemStt100Types.TranscriptionResult;
  export type SttError = golemStt100Types.SttError;
  export type LanguageCode = golemStt100Types.LanguageCode;
  export type Phrase = {
    value: string;
    boost?: number;
  };
  export type Vocabulary = {
    phrases: Phrase[];
  };
  export type DiarizationOptions = {
    enabled: boolean;
    minSpeakerCount?: number;
    maxSpeakerCount?: number;
  };
  export type TranscribeOptions = {
    language?: LanguageCode;
    model?: string;
    profanityFilter?: boolean;
    vocabulary?: Vocabulary;
    diarization?: DiarizationOptions;
    enableMultiChannel?: boolean;
  };
  export type TranscriptionRequest = {
    requestId: string;
    audio: Uint8Array;
    config: AudioConfig;
    options?: TranscribeOptions;
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
