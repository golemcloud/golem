/**
 * Connection management and graph instance creation
 */
declare module 'golem:graph/connection@1.0.0' {
  import * as golemGraph100Errors from 'golem:graph/errors@1.0.0';
  import * as golemGraph100Transactions from 'golem:graph/transactions@1.0.0';
  /**
   * Connect to a graph database with the specified configuration
   */
  export function connect(config: ConnectionConfig): Result<Graph, GraphError>;
  export class Graph {
    /**
     * Create a new transaction for performing operations
     */
    beginTransaction(): Result<Transaction, GraphError>;
    /**
     * Create a read-only transaction (may be optimized by provider)
     */
    beginReadTransaction(): Result<Transaction, GraphError>;
    /**
     * Test connection health
     */
    ping(): Result<void, GraphError>;
    /**
     * Close the graph connection
     */
    close(): Result<void, GraphError>;
    /**
     * Get basic graph statistics if supported
     */
    getStatistics(): Result<GraphStatistics, GraphError>;
  }
  export type GraphError = golemGraph100Errors.GraphError;
  export type Transaction = golemGraph100Transactions.Transaction;
  /**
   * Configuration for connecting to graph databases
   */
  export type ConnectionConfig = {
    hosts: string[];
    port: number | undefined;
    databaseName: string | undefined;
    username: string | undefined;
    password: string | undefined;
    timeoutSeconds: number | undefined;
    maxConnections: number | undefined;
    providerConfig: [string, string][];
  };
  /**
   * Basic graph statistics
   */
  export type GraphStatistics = {
    vertexCount: bigint | undefined;
    edgeCount: bigint | undefined;
    labelCount: number | undefined;
    propertyCount: bigint | undefined;
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
