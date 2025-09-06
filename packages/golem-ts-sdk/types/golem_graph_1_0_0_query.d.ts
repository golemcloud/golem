/**
 * Generic query interface for database-specific query languages
 */
declare module 'golem:graph/query@1.0.0' {
  import * as golemGraph100Errors from 'golem:graph/errors@1.0.0';
  import * as golemGraph100Transactions from 'golem:graph/transactions@1.0.0';
  import * as golemGraph100Types from 'golem:graph/types@1.0.0';
  /**
   * Execute a database-specific query string
   */
  export function executeQuery(transaction: Transaction, query: string, parameters: QueryParameters | undefined, options: QueryOptions | undefined): Result<QueryExecutionResult, GraphError>;
  export type Vertex = golemGraph100Types.Vertex;
  export type Edge = golemGraph100Types.Edge;
  export type Path = golemGraph100Types.Path;
  export type PropertyValue = golemGraph100Types.PropertyValue;
  export type GraphError = golemGraph100Errors.GraphError;
  export type Transaction = golemGraph100Transactions.Transaction;
  /**
   * Query result that maintains symmetry with data insertion formats
   */
  export type QueryResult = {
    tag: 'vertices'
    val: Vertex[]
  } |
  {
    tag: 'edges'
    val: Edge[]
  } |
  {
    tag: 'paths'
    val: Path[]
  } |
  {
    tag: 'values'
    val: PropertyValue[]
  } |
  {
    tag: 'maps'
    val: [string, PropertyValue][][]
  };
  /**
   * Query parameters for parameterized queries
   */
  export type QueryParameters = [string, PropertyValue][];
  /**
   * Query execution options
   */
  export type QueryOptions = {
    timeoutSeconds: number | undefined;
    maxResults: number | undefined;
    explain: boolean;
    profile: boolean;
  };
  /**
   * Query execution result with metadata
   */
  export type QueryExecutionResult = {
    queryResultValue: QueryResult;
    executionTimeMs: number | undefined;
    rowsAffected: number | undefined;
    explanation: string | undefined;
    profileData: string | undefined;
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
