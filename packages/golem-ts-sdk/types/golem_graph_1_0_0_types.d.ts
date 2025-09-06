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
    day: number;
  };
  export type Time = {
    hour: number;
    minute: number;
    second: number;
    nanosecond: number;
  };
  export type Datetime = {
    date: Date;
    time: Time;
    timezoneOffsetMinutes: number | undefined;
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
    altitude: number | undefined;
  };
  export type Linestring = {
    coordinates: Point[];
  };
  export type Polygon = {
    exterior: Point[];
    holes: Point[][] | undefined;
  };
  /**
   * Universal property value types that can be represented across all graph databases
   */
  export type PropertyValue = {
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
  export type ElementId = {
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
    additionalLabels: string[];
    properties: PropertyMap;
  };
  /**
   * Edge representation
   */
  export type Edge = {
    id: ElementId;
    edgeType: string;
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
}
