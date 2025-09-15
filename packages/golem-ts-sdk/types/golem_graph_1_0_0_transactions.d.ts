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
     * @throws GraphError
     */
    createVertex(vertexType: string, properties: PropertyMap): Vertex;
    /**
     * Create vertex with additional labels (for multi-label systems like Neo4j)
     * @throws GraphError
     */
    createVertexWithLabels(vertexType: string, additionalLabels: string[], properties: PropertyMap): Vertex;
    /**
     * Get vertex by ID
     * @throws GraphError
     */
    getVertex(id: ElementId): Vertex | undefined;
    /**
     * Update vertex properties (replaces all properties)
     * @throws GraphError
     */
    updateVertex(id: ElementId, properties: PropertyMap): Vertex;
    /**
     * Update specific vertex properties (partial update)
     * @throws GraphError
     */
    updateVertexProperties(id: ElementId, updates: PropertyMap): Vertex;
    /**
     * Delete vertex (and optionally its edges)
     * @throws GraphError
     */
    deleteVertex(id: ElementId, deleteEdges: boolean): void;
    /**
     * Find vertices by type and optional filters
     * @throws GraphError
     */
    findVertices(vertexType: string | undefined, filters: FilterCondition[] | undefined, sort: SortSpec[] | undefined, limit: number | undefined, offset: number | undefined): Vertex[];
    /**
     * === EDGE OPERATIONS ===
     * Create a new edge
     * @throws GraphError
     */
    createEdge(edgeType: string, fromVertex: ElementId, toVertex: ElementId, properties: PropertyMap): Edge;
    /**
     * Get edge by ID
     * @throws GraphError
     */
    getEdge(id: ElementId): Edge | undefined;
    /**
     * Update edge properties
     * @throws GraphError
     */
    updateEdge(id: ElementId, properties: PropertyMap): Edge;
    /**
     * Update specific edge properties (partial update)
     * @throws GraphError
     */
    updateEdgeProperties(id: ElementId, updates: PropertyMap): Edge;
    /**
     * Delete edge
     * @throws GraphError
     */
    deleteEdge(id: ElementId): void;
    /**
     * Find edges by type and optional filters
     * @throws GraphError
     */
    findEdges(edgeTypes: string[] | undefined, filters: FilterCondition[] | undefined, sort: SortSpec[] | undefined, limit: number | undefined, offset: number | undefined): Edge[];
    /**
     * === TRAVERSAL OPERATIONS ===
     * Get adjacent vertices through specified edge types
     * @throws GraphError
     */
    getAdjacentVertices(vertexId: ElementId, direction: Direction, edgeTypes: string[] | undefined, limit: number | undefined): Vertex[];
    /**
     * Get edges connected to a vertex
     * @throws GraphError
     */
    getConnectedEdges(vertexId: ElementId, direction: Direction, edgeTypes: string[] | undefined, limit: number | undefined): Edge[];
    /**
     * === BATCH OPERATIONS ===
     * Create multiple vertices in a single operation
     * @throws GraphError
     */
    createVertices(vertices: VertexSpec[]): Vertex[];
    /**
     * Create multiple edges in a single operation
     * @throws GraphError
     */
    createEdges(edges: EdgeSpec[]): Edge[];
    /**
     * Upsert vertex (create or update)
     * @throws GraphError
     */
    upsertVertex(id: ElementId | undefined, vertexType: string, properties: PropertyMap): Vertex;
    /**
     * Upsert edge (create or update)
     * @throws GraphError
     */
    upsertEdge(id: ElementId | undefined, edgeType: string, fromVertex: ElementId, toVertex: ElementId, properties: PropertyMap): Edge;
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
  export type GraphError = golemGraph100Errors.GraphError;
  /**
   * Vertex specification for batch creation
   */
  export type VertexSpec = {
    vertexType: string;
    additionalLabels?: string[];
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
