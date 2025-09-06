/**
 * Schema management operations (optional/emulated for schema-free databases)
 */
declare module 'golem:graph/schema@1.0.0' {
  import * as golemGraph100Connection from 'golem:graph/connection@1.0.0';
  import * as golemGraph100Errors from 'golem:graph/errors@1.0.0';
  import * as golemGraph100Types from 'golem:graph/types@1.0.0';
  /**
   * Get schema manager for the graph
   */
  export function getSchemaManager(config: ConnectionConfig | undefined): Result<SchemaManager, GraphError>;
  export class SchemaManager {
    /**
     * Define or update vertex label schema
     */
    defineVertexLabel(schema: VertexLabelSchema): Result<void, GraphError>;
    /**
     * Define or update edge label schema
     */
    defineEdgeLabel(schema: EdgeLabelSchema): Result<void, GraphError>;
    /**
     * Get vertex label schema
     */
    getVertexLabelSchema(label: string): Result<VertexLabelSchema | undefined, GraphError>;
    /**
     * Get edge label schema
     */
    getEdgeLabelSchema(label: string): Result<EdgeLabelSchema | undefined, GraphError>;
    /**
     * List all vertex labels
     */
    listVertexLabels(): Result<string[], GraphError>;
    /**
     * List all edge labels
     */
    listEdgeLabels(): Result<string[], GraphError>;
    /**
     * Create index
     */
    createIndex(index: IndexDefinition): Result<void, GraphError>;
    /**
     * Drop index
     */
    dropIndex(name: string): Result<void, GraphError>;
    /**
     * List indexes
     */
    listIndexes(): Result<IndexDefinition[], GraphError>;
    /**
     * Get index by name
     */
    getIndex(name: string): Result<IndexDefinition | undefined, GraphError>;
    /**
     * Define edge type for structural databases (ArangoDB-style)
     */
    defineEdgeType(definition: EdgeTypeDefinition): Result<void, GraphError>;
    /**
     * List edge type definitions
     */
    listEdgeTypes(): Result<EdgeTypeDefinition[], GraphError>;
    /**
     * Create container/collection for organizing data
     */
    createContainer(name: string, containerType: ContainerType): Result<void, GraphError>;
    /**
     * List containers/collections
     */
    listContainers(): Result<ContainerInfo[], GraphError>;
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
    defaultValue: PropertyValue | undefined;
  };
  /**
   * Vertex label schema
   */
  export type VertexLabelSchema = {
    label: string;
    properties: PropertyDefinition[];
    container: string | undefined;
  };
  /**
   * Edge label schema
   */
  export type EdgeLabelSchema = {
    label: string;
    properties: PropertyDefinition[];
    fromLabels: string[] | undefined;
    toLabels: string[] | undefined;
    container: string | undefined;
  };
  /**
   * Index definition
   */
  export type IndexDefinition = {
    name: string;
    label: string;
    properties: string[];
    indexType: IndexType;
    unique: boolean;
    container: string | undefined;
  };
  /**
   * Definition for an edge type in a structural graph database.
   */
  export type EdgeTypeDefinition = {
    collection: string;
    fromCollections: string[];
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
    elementCount: bigint | undefined;
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
