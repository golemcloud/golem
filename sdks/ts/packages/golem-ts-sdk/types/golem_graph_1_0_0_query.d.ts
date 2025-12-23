/**
 * Generic query types for database-specific query languages
 */
declare module 'golem:graph/query@1.0.0' {
  import * as golemGraph100Errors from 'golem:graph/errors@1.0.0';
  import * as golemGraph100Types from 'golem:graph/types@1.0.0';
  export type Vertex = golemGraph100Types.Vertex;
  export type Edge = golemGraph100Types.Edge;
  export type Path = golemGraph100Types.Path;
  export type PropertyValue = golemGraph100Types.PropertyValue;
  export type GraphError = golemGraph100Errors.GraphError;
}
