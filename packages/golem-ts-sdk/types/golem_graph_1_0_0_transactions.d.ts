/**
 * All graph operations performed within transaction contexts
 */
declare module 'golem:graph/transactions@1.0.0' {
  import * as golemGraph100Errors from 'golem:graph/errors@1.0.0';
  import * as golemGraph100Types from 'golem:graph/types@1.0.0';
  export class Transaction {
    /**
     * === VERTEX OPERATIONS ===
     * Create a new vertex
     */
    createVertex(vertexType: string, properties: PropertyMap): Result<Vertex, GraphError>;
    /**
     * Create vertex with additional labels (for multi-label systems like Neo4j)
     */
    createVertexWithLabels(vertexType: string, additionalLabels: string[], properties: PropertyMap): Result<Vertex, GraphError>;
    /**
     * Get vertex by ID
     */
    getVertex(id: ElementId): Result<Vertex | undefined, GraphError>;
    /**
     * Update vertex properties (replaces all properties)
     */
    updateVertex(id: ElementId, properties: PropertyMap): Result<Vertex, GraphError>;
    /**
     * Update specific vertex properties (partial update)
     */
    updateVertexProperties(id: ElementId, updates: PropertyMap): Result<Vertex, GraphError>;
    /**
     * Delete vertex (and optionally its edges)
     */
    deleteVertex(id: ElementId, deleteEdges: boolean): Result<void, GraphError>;
    /**
     * Find vertices by type and optional filters
     */
    findVertices(vertexType: string | undefined, filters: FilterCondition[] | undefined, sort: SortSpec[] | undefined, limit: number | undefined, offset: number | undefined): Result<Vertex[], GraphError>;
    /**
     * === EDGE OPERATIONS ===
     * Create a new edge
     */
    createEdge(edgeType: string, fromVertex: ElementId, toVertex: ElementId, properties: PropertyMap): Result<Edge, GraphError>;
    /**
     * Get edge by ID
     */
    getEdge(id: ElementId): Result<Edge | undefined, GraphError>;
    /**
     * Update edge properties
     */
    updateEdge(id: ElementId, properties: PropertyMap): Result<Edge, GraphError>;
    /**
     * Update specific edge properties (partial update)
     */
    updateEdgeProperties(id: ElementId, updates: PropertyMap): Result<Edge, GraphError>;
    /**
     * Delete edge
     */
    deleteEdge(id: ElementId): Result<void, GraphError>;
    /**
     * Find edges by type and optional filters
     */
    findEdges(edgeTypes: string[] | undefined, filters: FilterCondition[] | undefined, sort: SortSpec[] | undefined, limit: number | undefined, offset: number | undefined): Result<Edge[], GraphError>;
    /**
     * === TRAVERSAL OPERATIONS ===
     * Get adjacent vertices through specified edge types
     */
    getAdjacentVertices(vertexId: ElementId, direction: Direction, edgeTypes: string[] | undefined, limit: number | undefined): Result<Vertex[], GraphError>;
    /**
     * Get edges connected to a vertex
     */
    getConnectedEdges(vertexId: ElementId, direction: Direction, edgeTypes: string[] | undefined, limit: number | undefined): Result<Edge[], GraphError>;
    /**
     * === BATCH OPERATIONS ===
     * Create multiple vertices in a single operation
     */
    createVertices(vertices: VertexSpec[]): Result<Vertex[], GraphError>;
    /**
     * Create multiple edges in a single operation
     */
    createEdges(edges: EdgeSpec[]): Result<Edge[], GraphError>;
    /**
     * Upsert vertex (create or update)
     */
    upsertVertex(id: ElementId | undefined, vertexType: string, properties: PropertyMap): Result<Vertex, GraphError>;
    /**
     * Upsert edge (create or update)
     */
    upsertEdge(id: ElementId | undefined, edgeType: string, fromVertex: ElementId, toVertex: ElementId, properties: PropertyMap): Result<Edge, GraphError>;
    /**
     * === TRANSACTION CONTROL ===
     * Commit the transaction
     */
    commit(): Result<void, GraphError>;
    /**
     * Rollback the transaction
     */
    rollback(): Result<void, GraphError>;
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
  export type GraphError = golemGraph100Errors.GraphError;
  /**
   * Vertex specification for batch creation
   */
  export type VertexSpec = {
    vertexType: string;
    additionalLabels: string[] | undefined;
    properties: PropertyMap;
  };
  /**
   * Edge specification for batch creation
   */
  export type EdgeSpec = {
    edgeType: string;
    fromVertex: ElementId;
    toVertex: ElementId;
    properties: PropertyMap;
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
