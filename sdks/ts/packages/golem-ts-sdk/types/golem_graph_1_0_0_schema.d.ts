/**
 * Schema management operations (optional/emulated for schema-free databases)
 */
declare module 'golem:graph/schema@1.0.0' {
  import * as golemGraph100Connection from 'golem:graph/connection@1.0.0';
  import * as golemGraph100Errors from 'golem:graph/errors@1.0.0';
  import * as golemGraph100Types from 'golem:graph/types@1.0.0';
  /**
   * Get schema manager for the graph
   * @throws GraphError
   */
  export function getSchemaManager(config: ConnectionConfig | undefined): SchemaManager;
  export class SchemaManager {
    /**
     * Define or update vertex label schema
     * @throws GraphError
     */
    defineVertexLabel(schema: VertexLabelSchema): void;
    /**
     * Define or update edge label schema
     * @throws GraphError
     */
    defineEdgeLabel(schema: EdgeLabelSchema): void;
    /**
     * Get vertex label schema
     * @throws GraphError
     */
    getVertexLabelSchema(label: string): VertexLabelSchema | undefined;
    /**
     * Get edge label schema
     * @throws GraphError
     */
    getEdgeLabelSchema(label: string): EdgeLabelSchema | undefined;
    /**
     * List all vertex labels
     * @throws GraphError
     */
    listVertexLabels(): string[];
    /**
     * List all edge labels
     * @throws GraphError
     */
    listEdgeLabels(): string[];
    /**
     * Create index
     * @throws GraphError
     */
    createIndex(index: IndexDefinition): void;
    /**
     * Drop index
     * @throws GraphError
     */
    dropIndex(name: string): void;
    /**
     * List indexes
     * @throws GraphError
     */
    listIndexes(): IndexDefinition[];
    /**
     * Get index by name
     * @throws GraphError
     */
    getIndex(name: string): IndexDefinition | undefined;
    /**
     * Define edge type for structural databases (ArangoDB-style)
     * @throws GraphError
     */
    defineEdgeType(definition: EdgeTypeDefinition): void;
    /**
     * List edge type definitions
     * @throws GraphError
     */
    listEdgeTypes(): EdgeTypeDefinition[];
    /**
     * Create container/collection for organizing data
     * @throws GraphError
     */
    createContainer(name: string, containerType: ContainerType): void;
    /**
     * List containers/collections
     * @throws GraphError
     */
    listContainers(): ContainerInfo[];
  }
  export type PropertyValue = golemGraph100Types.PropertyValue;
  export type GraphError = golemGraph100Errors.GraphError;
  export type ConnectionConfig = golemGraph100Connection.ConnectionConfig;
  /**
   * Property type definitions for schema
   */
  export type PropertyType = "boolean" | "int32" | "int64" | "float32-type" | "float64-type" | "string-type" | "bytes" | "date" | "datetime" | "point" | "list-type" | "map-type";
  /**
   * Index types
   */
  export type IndexType = "exact" | "range" | "text" | "geospatial";
  /**
   * Property definition for schema
   */
  export type PropertyDefinition = {
    name: string;
    propertyType: PropertyType;
    required: boolean;
    unique: boolean;
    defaultValue?: PropertyValue;
  };
  /**
   * Vertex label schema
   */
  export type VertexLabelSchema = {
    label: string;
    properties: PropertyDefinition[];
    /** Container/collection this label maps to (for container-based systems) */
    container?: string;
  };
  /**
   * Edge label schema
   */
  export type EdgeLabelSchema = {
    label: string;
    properties: PropertyDefinition[];
    fromLabels?: string[];
    /** Allowed source vertex labels */
    toLabels?: string[];
    /**
     * Allowed target vertex labels
     * Container/collection this label maps to (for container-based systems)
     */
    container?: string;
  };
  /**
   * Index definition
   */
  export type IndexDefinition = {
    name: string;
    label: string;
    /** Vertex or edge label */
    properties: string[];
    /** Properties to index */
    indexType: IndexType;
    unique: boolean;
    /** Container/collection this index applies to */
    container?: string;
  };
  /**
   * Definition for an edge type in a structural graph database.
   */
  export type EdgeTypeDefinition = {
    /** The name of the edge collection/table. */
    collection: string;
    /** The names of vertex collections/tables that can be at the 'from' end of an edge. */
    fromCollections: string[];
    /** The names of vertex collections/tables that can be at the 'to' end of an edge. */
    toCollections: string[];
  };
  /**
   * Container/collection types
   */
  export type ContainerType = "vertex-container" | "edge-container";
  /**
   * Container information
   */
  export type ContainerInfo = {
    name: string;
    containerType: ContainerType;
    elementCount?: bigint;
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
