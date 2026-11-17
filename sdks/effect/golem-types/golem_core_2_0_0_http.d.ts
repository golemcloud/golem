declare module 'golem:core/http@2.0.0' {
  export type HttpMethod =
  {
    tag: 'get'
  } |
  {
    tag: 'head'
  } |
  {
    tag: 'post'
  } |
  {
    tag: 'put'
  } |
  {
    tag: 'delete'
  } |
  {
    tag: 'connect'
  } |
  {
    tag: 'options'
  } |
  {
    tag: 'trace'
  } |
  {
    tag: 'patch'
  } |
  {
    tag: 'custom'
    val: string
  };
  export type PathVariable = {
    variableName: string;
  };
  export type PathSegment =
  {
    tag: 'literal'
    val: string
  } |
  {
    tag: 'path-variable'
    val: PathVariable
  } |
  /** only allowed as the last segment */
  {
    tag: 'remaining-path-variable'
    val: PathVariable
  };
  export type HeaderVariable = {
    headerName: string;
    variableName: string;
  };
  export type QueryVariable = {
    queryParamName: string;
    variableName: string;
  };
  export type ExactFileMapping = {
    publicPath: string[];
    filePath: string;
  };
  export type SubtreeFileMapping = {
    publicPrefix: string[];
    filesystemRoot: string;
  };
  export type FileMapping =
  {
    tag: 'exact'
    val: ExactFileMapping
  } |
  {
    tag: 'subtree'
    val: SubtreeFileMapping
  };
  export type FileResponseHeader = {
    name: string;
    value: string;
  };
  export type HttpRouteMode = "durable-streams" | "plain" | "sse" | "web-socket";
  export type DurableStreamSlotSource =
  {
    tag: 'input'
    val: string
  } |
  {
    tag: 'output'
    val: string
  };
  export type DurableStreamSlotOptions = {
    source: DurableStreamSlotSource;
    name?: string;
    contentType?: string;
  };
  export type DurableStreamRouteLoadOptions = {
    maxConcurrentReadersPerStream?: number;
    maxAppendRequestsPerSecondPerStream?: number;
  };
  export type DurableStreamRouteOptions = {
    slots: DurableStreamSlotOptions[];
    load?: DurableStreamRouteLoadOptions;
  };
}
