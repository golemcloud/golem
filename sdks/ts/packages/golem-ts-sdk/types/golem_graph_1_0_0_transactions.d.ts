/**
 * All graph operations performed within transaction contexts
 */
declare module 'golem:graph/transactions@1.0.0' {
  import * as golemGraph100Errors from 'golem:graph/errors@1.0.0';
  import * as golemGraph100Types from 'golem:graph/types@1.0.0';
  export class Transaction {
    /**
     * === QUERY OPERATIONS ===
     * Execute a database-specific query string
     * @throws GraphError
     */
    executeQuery(options: ExecuteQueryOptions): QueryExecutionResult;
    /**
     * == TRAVESAL OPERATIONS ==
     * Find shortest path between two vertices
     * @throws GraphError
     */
    findShortestPath(options: FindShortestPathOptions): Path | undefined;
    /**
     * Find all paths between two vertices (up to limit)
     * @throws GraphError
     */
    findAllPaths(options: FindAllPathsOptions): Path[];
    /**
     * Get k-hop neighborhood around a vertex
     * @throws GraphError
     */
    getNeighborhood(options: GetNeighborhoodOptions): Subgraph;
    /**
     * Check if path exists between vertices
     * @throws GraphError
     */
    pathExists(options: PathExistsOptions): boolean;
    /**
     * Get vertices at specific distance from source
     * @throws GraphError
     */
    getVerticesAtDistance(options: GetVerticesAtDistanceOptions): Vertex[];
    /**
     * Get adjacent vertices through specified edge types
     * @throws GraphError
     */
    getAdjacentVertices(options: GetAdjacentVerticesOptions): Vertex[];
    /**
     * Get edges connected to a vertex
     * @throws GraphError
     */
    getConnectedEdges(options: GetConnectedEdgesOptions): Edge[];
    /**
     * === VERTEX OPERATIONS ===
     * Create a new vertex
     * @throws GraphError
     */
    createVertex(options: CreateVertexOptions): Vertex;
    /**
     * Create multiple vertices in a single operation
     * @throws GraphError
     */
    createVertices(vertices: CreateVertexOptions[]): Vertex[];
    /**
     * Get vertex by ID
     * @throws GraphError
     */
    getVertex(id: ElementId): Vertex | undefined;
    /**
     * Update vertex properties (replaces all properties)
     * @throws GraphError
     */
    updateVertex(options: UpdateVertexOptions): Vertex;
    /**
     * Delete vertex (and optionally its edges)
     * @throws GraphError
     */
    deleteVertex(id: ElementId, deleteEdges: boolean): void;
    /**
     * Find vertices by type and optional filters
     * @throws GraphError
     */
    findVertices(options: FindVerticesOptions): Vertex[];
    /**
     * === EDGE OPERATIONS ===
     * Create a new edge
     * @throws GraphError
     */
    createEdge(options: CreateEdgeOptions): Edge;
    /**
     * Create multiple edges in a single operation
     * @throws GraphError
     */
    createEdges(edges: CreateEdgeOptions[]): Edge[];
    /**
     * Get edge by ID
     * @throws GraphError
     */
    getEdge(id: ElementId): Edge | undefined;
    /**
     * Update edge properties
     * @throws GraphError
     */
    updateEdge(options: UpdateEdgeOptions): Edge;
    /**
     * Delete edge
     * @throws GraphError
     */
    deleteEdge(id: ElementId): void;
    /**
     * Find edges by type and optional filters
     * @throws GraphError
     */
    findEdges(options: FindEdgesOptions): Edge[];
    /**
     * === TRANSACTION CONTROL ===
     * Commit the transaction
     * @throws GraphError
     */
    commit(): void;
    /**
     * Rollback the transaction
     * @throws GraphError
     */
    rollback(): void;
    /**
     * Check if transaction is still active
     */
    isActive(): boolean;
  }
  export type Vertex = golemGraph100Types.Vertex;
  export type Edge = golemGraph100Types.Edge;
  export type Path = golemGraph100Types.Path;
  export type ElementId = golemGraph100Types.ElementId;
  export type PropertyMap = golemGraph100Types.PropertyMap;
  export type PropertyValue = golemGraph100Types.PropertyValue;
  export type FilterCondition = golemGraph100Types.FilterCondition;
  export type SortSpec = golemGraph100Types.SortSpec;
  export type Direction = golemGraph100Types.Direction;
  export type Subgraph = golemGraph100Types.Subgraph;
  export type ExecuteQueryOptions = golemGraph100Types.ExecuteQueryOptions;
  export type QueryExecutionResult = golemGraph100Types.QueryExecutionResult;
  export type FindShortestPathOptions = golemGraph100Types.FindShortestPathOptions;
  export type FindAllPathsOptions = golemGraph100Types.FindAllPathsOptions;
  export type FindEdgesOptions = golemGraph100Types.FindEdgesOptions;
  export type GetAdjacentVerticesOptions = golemGraph100Types.GetAdjacentVerticesOptions;
  export type GetConnectedEdgesOptions = golemGraph100Types.GetConnectedEdgesOptions;
  export type GetVerticesAtDistanceOptions = golemGraph100Types.GetVerticesAtDistanceOptions;
  export type FindVerticesOptions = golemGraph100Types.FindVerticesOptions;
  export type GetNeighborhoodOptions = golemGraph100Types.GetNeighborhoodOptions;
  export type PathExistsOptions = golemGraph100Types.PathExistsOptions;
  export type CreateVertexOptions = golemGraph100Types.CreateVertexOptions;
  export type UpdateVertexOptions = golemGraph100Types.UpdateVertexOptions;
  export type CreateEdgeOptions = golemGraph100Types.CreateEdgeOptions;
  export type UpdateEdgeOptions = golemGraph100Types.UpdateEdgeOptions;
  export type GraphError = golemGraph100Errors.GraphError;
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
