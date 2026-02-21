/**
 * Core data types and structures unified across graph databases
 */
declare module 'golem:graph/types@1.0.0' {
  /**
   * Temporal types with unified representation
   */
  export type Date = {
    year: number;
    month: number;
    /** 1-12 */
    day: number;
  };
  export type Time = {
    hour: number;
    /** 0-23 */
    minute: number;
    /** 0-59 */
    second: number;
    /** 0-59 */
    nanosecond: number;
  };
  export type Datetime = {
    date: Date;
    time: Time;
    timezoneOffsetMinutes?: number;
  };
  export type Duration = {
    seconds: bigint;
    nanoseconds: number;
  };
  /**
   * Geospatial types (WGS84 coordinates)
   */
  export type Point = {
    longitude: number;
    latitude: number;
    altitude?: number;
  };
  export type Linestring = {
    coordinates: Point[];
  };
  export type Polygon = {
    exterior: Point[];
    holes?: Point[][];
  };
  /**
   * Universal property value types that can be represented across all graph databases
   */
  export type PropertyValue = 
  {
    tag: 'null-value'
  } |
  {
    tag: 'boolean'
    val: boolean
  } |
  {
    tag: 'int8'
    val: number
  } |
  {
    tag: 'int16'
    val: number
  } |
  {
    tag: 'int32'
    val: number
  } |
  {
    tag: 'int64'
    val: bigint
  } |
  {
    tag: 'uint8'
    val: number
  } |
  {
    tag: 'uint16'
    val: number
  } |
  {
    tag: 'uint32'
    val: number
  } |
  {
    tag: 'uint64'
    val: bigint
  } |
  {
    tag: 'float32-value'
    val: number
  } |
  {
    tag: 'float64-value'
    val: number
  } |
  {
    tag: 'string-value'
    val: string
  } |
  {
    tag: 'bytes'
    val: Uint8Array
  } |
  /** Temporal types (unified representation) */
  {
    tag: 'date'
    val: Date
  } |
  {
    tag: 'time'
    val: Time
  } |
  {
    tag: 'datetime'
    val: Datetime
  } |
  {
    tag: 'duration'
    val: Duration
  } |
  /** Geospatial types (unified GeoJSON-like representation) */
  {
    tag: 'point'
    val: Point
  } |
  {
    tag: 'linestring'
    val: Linestring
  } |
  {
    tag: 'polygon'
    val: Polygon
  };
  /**
   * Universal element ID that can represent various database ID schemes
   */
  export type ElementId = 
  {
    tag: 'string-value'
    val: string
  } |
  {
    tag: 'int64'
    val: bigint
  } |
  {
    tag: 'uuid'
    val: string
  };
  /**
   * Property map - consistent with insertion format
   */
  export type PropertyMap = [string, PropertyValue][];
  /**
   * Vertex representation
   */
  export type Vertex = {
    id: ElementId;
    vertexType: string;
    /** Primary type (collection/tag/label) */
    additionalLabels: string[];
    /** Secondary labels (Neo4j-style) */
    properties: PropertyMap;
  };
  /**
   * Edge representation
   */
  export type Edge = {
    id: ElementId;
    edgeType: string;
    /** Edge type/relationship type */
    fromVertex: ElementId;
    toVertex: ElementId;
    properties: PropertyMap;
  };
  /**
   * Path through the graph
   */
  export type Path = {
    vertices: Vertex[];
    edges: Edge[];
    length: number;
  };
  /**
   * Direction for traversals
   */
  export type Direction = "outgoing" | "incoming" | "both";
  /**
   * Comparison operators for filtering
   */
  export type ComparisonOperator = "equal" | "not-equal" | "less-than" | "less-than-or-equal" | "greater-than" | "greater-than-or-equal" | "contains" | "starts-with" | "ends-with" | "regex-match" | "in-list" | "not-in-list";
  /**
   * Filter condition for queries
   */
  export type FilterCondition = {
    property: string;
    operator: ComparisonOperator;
    value: PropertyValue;
  };
  /**
   * Sort specification
   */
  export type SortSpec = {
    property: string;
    ascending: boolean;
  };
  /**
   * Query result that maintains symmetry with data insertion formats
   */
  export type QueryResult = 
  {
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
   * Options for execute-query
   */
  export type ExecuteQueryOptions = {
    query: string;
    parameters?: QueryParameters;
    timeoutSeconds?: number;
    maxResults?: number;
    /** Return execution plan instead of results, default to false */
    explain?: boolean;
    /** Include performance metrics, default to false */
    profile?: boolean;
  };
  /**
   * Options for create-vertex
   */
  export type CreateVertexOptions = {
    vertexType: string;
    properties?: PropertyMap;
    /** For multi-label systems like Neo4j */
    labels?: string[];
  };
  /**
   * Options for update-vertex
   */
  export type UpdateVertexOptions = {
    id: ElementId;
    properties: PropertyMap;
    partial?: boolean;
    /** defaults to false */
    createMissing?: boolean;
  };
  /**
   * Options for create-edge
   */
  export type CreateEdgeOptions = {
    edgeType: string;
    fromVertex: ElementId;
    toVertex: ElementId;
    properties?: PropertyMap;
  };
  /**
   * Sub-options for update-edge
   */
  export type CreateMissingEdgeOptions = {
    edgeType: string;
    fromVertex: ElementId;
    toVertex: ElementId;
  };
  /**
   * Options for update-edge
   */
  export type UpdateEdgeOptions = {
    id: ElementId;
    properties: PropertyMap;
    partial?: boolean;
    /** defaults to false */
    createMissingWith?: CreateMissingEdgeOptions;
  };
  /**
   * Options for find-vertices
   */
  export type FindVerticesOptions = {
    vertexType?: string;
    filters?: FilterCondition[];
    sort?: SortSpec[];
    limit?: number;
    offset?: number;
  };
  /**
   * Options for find-edges
   */
  export type FindEdgesOptions = {
    edgeTypes?: string[];
    filters?: FilterCondition[];
    sort?: SortSpec[];
    limit?: number;
    offset?: number;
  };
  /**
   * Options for get-adjacent-vertices
   */
  export type GetAdjacentVerticesOptions = {
    vertexId: ElementId;
    direction: Direction;
    edgeTypes?: string[];
    limit?: number;
  };
  /**
   * Options for get-connected-edges
   */
  export type GetConnectedEdgesOptions = {
    vertexId: ElementId;
    direction: Direction;
    edgeTypes?: string[];
    limit?: number;
  };
  /**
   * Query execution result with metadata
   */
  export type QueryExecutionResult = {
    queryResultValue: QueryResult;
    executionTimeMs?: number;
    rowsAffected?: number;
    explanation?: string;
    /** Execution plan if requested */
    profileData?: string;
  };
  /**
   * Common path finding sub-options
   */
  export type PathOptions = {
    maxDepth?: number;
    edgeTypes?: string[];
    vertexTypes?: string[];
    vertexFilters?: FilterCondition[];
    edgeFilters?: FilterCondition[];
  };
  /**
   * Options for neighborhood
   */
  export type GetNeighborhoodOptions = {
    center: ElementId;
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
  /**
   * Options for find-shortest-path
   */
  export type FindShortestPathOptions = {
    fromVertex: ElementId;
    toVertex: ElementId;
    path?: PathOptions;
  };
  /**
   * Options for find-all-paths
   */
  export type FindAllPathsOptions = {
    fromVertex: ElementId;
    toVertex: ElementId;
    path?: PathOptions;
    limit?: number;
  };
  /**
   * Options for path-exists
   */
  export type PathExistsOptions = {
    fromVertex: ElementId;
    toVertex: ElementId;
    path?: PathOptions;
  };
  /**
   * Options for get-vertices-at-distance
   */
  export type GetVerticesAtDistanceOptions = {
    source: ElementId;
    distance: number;
    direction: Direction;
    edgeTypes?: string[];
  };
}
