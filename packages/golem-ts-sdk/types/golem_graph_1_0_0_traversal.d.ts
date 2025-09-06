/**
 * Graph traversal and pathfinding operations
 */
declare module 'golem:graph/traversal@1.0.0' {
  import * as golemGraph100Errors from 'golem:graph/errors@1.0.0';
  import * as golemGraph100Transactions from 'golem:graph/transactions@1.0.0';
  import * as golemGraph100Types from 'golem:graph/types@1.0.0';
  /**
   * Find shortest path between two vertices
   */
  export function findShortestPath(transaction: Transaction, fromVertex: ElementId, toVertex: ElementId, options: PathOptions | undefined): Result<Path | undefined, GraphError>;
  /**
   * Find all paths between two vertices (up to limit)
   */
  export function findAllPaths(transaction: Transaction, fromVertex: ElementId, toVertex: ElementId, options: PathOptions | undefined, limit: number | undefined): Result<Path[], GraphError>;
  /**
   * Get k-hop neighborhood around a vertex
   */
  export function getNeighborhood(transaction: Transaction, center: ElementId, options: NeighborhoodOptions): Result<Subgraph, GraphError>;
  /**
   * Check if path exists between vertices
   */
  export function pathExists(transaction: Transaction, fromVertex: ElementId, toVertex: ElementId, options: PathOptions | undefined): Result<boolean, GraphError>;
  /**
   * Get vertices at specific distance from source
   */
  export function getVerticesAtDistance(transaction: Transaction, source: ElementId, distance: number, direction: Direction, edgeTypes: string[] | undefined): Result<Vertex[], GraphError>;
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
    maxDepth: number | undefined;
    edgeTypes: string[] | undefined;
    vertexTypes: string[] | undefined;
    vertexFilters: FilterCondition[] | undefined;
    edgeFilters: FilterCondition[] | undefined;
  };
  /**
   * Neighborhood exploration options
   */
  export type NeighborhoodOptions = {
    depth: number;
    direction: Direction;
    edgeTypes: string[] | undefined;
    maxVertices: number | undefined;
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
