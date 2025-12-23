/**
 * Connection management and graph instance creation
 */
declare module 'golem:graph/connection@1.0.0' {
  import * as golemGraph100Errors from 'golem:graph/errors@1.0.0';
  import * as golemGraph100Transactions from 'golem:graph/transactions@1.0.0';
  /**
   * Connect to a graph database with the specified configuration
   * @throws GraphError
   */
  export function connect(config: ConnectionConfig): Graph;
  export class Graph {
    /**
     * Create a new transaction for performing operations
     * @throws GraphError
     */
    beginTransaction(): Transaction;
    /**
     * Create a read-only transaction (may be optimized by provider)
     * @throws GraphError
     */
    beginReadTransaction(): Transaction;
    /**
     * Test connection health
     * @throws GraphError
     */
    ping(): void;
    /**
     * Close the graph connection
     * @throws GraphError
     */
    close(): void;
    /**
     * Get basic graph statistics if supported
     * @throws GraphError
     */
    getStatistics(): GraphStatistics;
  }
  export type GraphError = golemGraph100Errors.GraphError;
  export type Transaction = golemGraph100Transactions.Transaction;
  /**
   * Configuration for connecting to graph databases
   */
  export type ConnectionConfig = {
    /** Connection parameters */
    hosts?: string[];
    port?: number;
    databaseName?: string;
    /** Authentication */
    username?: string;
    password?: string;
    /** Connection behavior */
    timeoutSeconds?: number;
    maxConnections?: number;
    /** Provider-specific configuration as key-value pairs */
    providerConfig: [string, string][];
  };
  /**
   * Basic graph statistics
   */
  export type GraphStatistics = {
    vertexCount?: bigint;
    edgeCount?: bigint;
    labelCount?: number;
    propertyCount?: bigint;
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
