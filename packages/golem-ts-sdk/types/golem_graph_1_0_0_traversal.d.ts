/**
 * Graph traversal and pathfinding operations
 */
declare module 'golem:graph/traversal@1.0.0' {
  import * as golemGraph100Errors from 'golem:graph/errors@1.0.0';
  import * as golemGraph100Transactions from 'golem:graph/transactions@1.0.0';
  import * as golemGraph100Types from 'golem:graph/types@1.0.0';
  /**
   * Find shortest path between two vertices
   * @throws GraphError
   */
  export function findShortestPath(transaction: Transaction, fromVertex: ElementId, toVertex: ElementId, options: PathOptions | undefined): Path | undefined;
  /**
   * Find all paths between two vertices (up to limit)
   * @throws GraphError
   */
  export function findAllPaths(transaction: Transaction, fromVertex: ElementId, toVertex: ElementId, options: PathOptions | undefined, limit: number | undefined): Path[];
  /**
   * Get k-hop neighborhood around a vertex
   * @throws GraphError
   */
  export function getNeighborhood(transaction: Transaction, center: ElementId, options: NeighborhoodOptions): Subgraph;
  /**
   * Check if path exists between vertices
   * @throws GraphError
   */
  export function pathExists(transaction: Transaction, fromVertex: ElementId, toVertex: ElementId, options: PathOptions | undefined): boolean;
  /**
   * Get vertices at specific distance from source
   * @throws GraphError
   */
  export function getVerticesAtDistance(transaction: Transaction, source: ElementId, distance: number, direction: Direction, edgeTypes: string[] | undefined): Vertex[];
  export type Vertex = golemGraph100Types.Vertex;
  export type Edge = golemGraph100Types.Edge;
  export type Path = golemGraph100Types.Path;
  export type ElementId = golemGraph100Types.ElementId;
  export type Direction = golemGraph100Types.Direction;
  export type FilterCondition = golemGraph100Types.FilterCondition;
  export type GraphError = golemGraph100Errors.GraphError;
  export type Transaction = golemGraph100Transactions.Transaction;
  /**
   * Path finding options
   */
  export type PathOptions = {
    maxDepth?: number;
    edgeTypes?: string[];
    vertexTypes?: string[];
    vertexFilters?: FilterCondition[];
    edgeFilters?: FilterCondition[];
  };
  /**
   * Neighborhood exploration options
   */
  export type NeighborhoodOptions = {
    depth: number;
    direction: Direction;
    edgeTypes?: string[];
    maxVertices?: number;
  };
  /**
   * Subgraph containing related vertices and edges
   */
  export type Subgraph = {
    vertices: Vertex[];
    edges: Edge[];
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
