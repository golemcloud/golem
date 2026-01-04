/**
 * Graph traversal and pathfinding types
 */
declare module 'golem:graph/traversal@1.0.0' {
  import * as golemGraph100Errors from 'golem:graph/errors@1.0.0';
  import * as golemGraph100Transactions from 'golem:graph/transactions@1.0.0';
  import * as golemGraph100Types from 'golem:graph/types@1.0.0';
  export type Vertex = golemGraph100Types.Vertex;
  export type Edge = golemGraph100Types.Edge;
  export type Path = golemGraph100Types.Path;
  export type ElementId = golemGraph100Types.ElementId;
  export type Direction = golemGraph100Types.Direction;
  export type FilterCondition = golemGraph100Types.FilterCondition;
  export type GraphError = golemGraph100Errors.GraphError;
  export type Transaction = golemGraph100Transactions.Transaction;
}
