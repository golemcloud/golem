declare module 'golem:stt/languages@1.0.0' {
  import * as golemStt100Types from 'golem:stt/types@1.0.0';
  export function listLanguages(): Result<LanguageInfo[], SttError>;
  export type LanguageCode = golemStt100Types.LanguageCode;
  export type SttError = golemStt100Types.SttError;
  export type LanguageInfo = {
    code: LanguageCode;
    name: string;
    nativeName: string;
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
