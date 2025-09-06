declare module 'golem:rdbms/postgres@0.0.1' {
  import * as golemRdbms001Types from 'golem:rdbms/types@0.0.1';
  export class LazyDbValue {
    constructor(value: DbValue);
    get(): DbValue;
  }
  export class LazyDbColumnType {
    constructor(value: DbColumnType);
    get(): DbColumnType;
  }
  export class DbResultStream {
    getColumns(): DbColumn[];
    getNext(): DbRow[] | undefined;
  }
  export class DbConnection {
    static open(address: string): Result<DbConnection, Error>;
    query(statement: string, params: DbValue[]): Result<DbResult, Error>;
    queryStream(statement: string, params: DbValue[]): Result<DbResultStream, Error>;
    execute(statement: string, params: DbValue[]): Result<bigint, Error>;
    beginTransaction(): Result<DbTransaction, Error>;
  }
  export class DbTransaction {
    query(statement: string, params: DbValue[]): Result<DbResult, Error>;
    queryStream(statement: string, params: DbValue[]): Result<DbResultStream, Error>;
    execute(statement: string, params: DbValue[]): Result<bigint, Error>;
    commit(): Result<void, Error>;
    rollback(): Result<void, Error>;
  }
  export type Date = golemRdbms001Types.Date;
  export type Time = golemRdbms001Types.Time;
  export type Timetz = golemRdbms001Types.Timetz;
  export type Timestamp = golemRdbms001Types.Timestamp;
  export type Timestamptz = golemRdbms001Types.Timestamptz;
  export type Uuid = golemRdbms001Types.Uuid;
  export type IpAddress = golemRdbms001Types.IpAddress;
  export type MacAddress = golemRdbms001Types.MacAddress;
  export type Error = {
    tag: 'connection-failure'
    val: string
  } |
  {
    tag: 'query-parameter-failure'
    val: string
  } |
  {
    tag: 'query-execution-failure'
    val: string
  } |
  {
    tag: 'query-response-failure'
    val: string
  } |
  {
    tag: 'other'
    val: string
  };
  export type Interval = {
    months: number;
    days: number;
    microseconds: bigint;
  };
  export type Int4bound = {
    tag: 'included'
    val: number
  } |
  {
    tag: 'excluded'
    val: number
  } |
  {
    tag: 'unbounded'
  };
  export type Int8bound = {
    tag: 'included'
    val: bigint
  } |
  {
    tag: 'excluded'
    val: bigint
  } |
  {
    tag: 'unbounded'
  };
  export type Numbound = {
    tag: 'included'
    val: string
  } |
  {
    tag: 'excluded'
    val: string
  } |
  {
    tag: 'unbounded'
  };
  export type Tsbound = {
    tag: 'included'
    val: Timestamp
  } |
  {
    tag: 'excluded'
    val: Timestamp
  } |
  {
    tag: 'unbounded'
  };
  export type Tstzbound = {
    tag: 'included'
    val: Timestamptz
  } |
  {
    tag: 'excluded'
    val: Timestamptz
  } |
  {
    tag: 'unbounded'
  };
  export type Datebound = {
    tag: 'included'
    val: Date
  } |
  {
    tag: 'excluded'
    val: Date
  } |
  {
    tag: 'unbounded'
  };
  export type Int4range = {
    start: Int4bound;
    end: Int4bound;
  };
  export type Int8range = {
    start: Int8bound;
    end: Int8bound;
  };
  export type Numrange = {
    start: Numbound;
    end: Numbound;
  };
  export type Tsrange = {
    start: Tsbound;
    end: Tsbound;
  };
  export type Tstzrange = {
    start: Tstzbound;
    end: Tstzbound;
  };
  export type Daterange = {
    start: Datebound;
    end: Datebound;
  };
  export type EnumerationType = {
    name: string;
  };
  export type Enumeration = {
    name: string;
    value: string;
  };
  export type Composite = {
    name: string;
    values: LazyDbValue[];
  };
  export type Domain = {
    name: string;
    value: LazyDbValue;
  };
  export type ValueBound = {
    tag: 'included'
    val: LazyDbValue
  } |
  {
    tag: 'excluded'
    val: LazyDbValue
  } |
  {
    tag: 'unbounded'
  };
  export type ValuesRange = {
    start: ValueBound;
    end: ValueBound;
  };
  export type Range = {
    name: string;
    value: ValuesRange;
  };
  export type DbValue = {
    tag: 'character'
    val: number
  } |
  {
    tag: 'int2'
    val: number
  } |
  {
    tag: 'int4'
    val: number
  } |
  {
    tag: 'int8'
    val: bigint
  } |
  {
    tag: 'float4'
    val: number
  } |
  {
    tag: 'float8'
    val: number
  } |
  {
    tag: 'numeric'
    val: string
  } |
  {
    tag: 'boolean'
    val: boolean
  } |
  {
    tag: 'text'
    val: string
  } |
  {
    tag: 'varchar'
    val: string
  } |
  {
    tag: 'bpchar'
    val: string
  } |
  {
    tag: 'timestamp'
    val: Timestamp
  } |
  {
    tag: 'timestamptz'
    val: Timestamptz
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
    tag: 'timetz'
    val: Timetz
  } |
  {
    tag: 'interval'
    val: Interval
  } |
  {
    tag: 'bytea'
    val: Uint8Array
  } |
  {
    tag: 'json'
    val: string
  } |
  {
    tag: 'jsonb'
    val: string
  } |
  {
    tag: 'jsonpath'
    val: string
  } |
  {
    tag: 'xml'
    val: string
  } |
  {
    tag: 'uuid'
    val: Uuid
  } |
  {
    tag: 'inet'
    val: IpAddress
  } |
  {
    tag: 'cidr'
    val: IpAddress
  } |
  {
    tag: 'macaddr'
    val: MacAddress
  } |
  {
    tag: 'bit'
    val: boolean[]
  } |
  {
    tag: 'varbit'
    val: boolean[]
  } |
  {
    tag: 'int4range'
    val: Int4range
  } |
  {
    tag: 'int8range'
    val: Int8range
  } |
  {
    tag: 'numrange'
    val: Numrange
  } |
  {
    tag: 'tsrange'
    val: Tsrange
  } |
  {
    tag: 'tstzrange'
    val: Tstzrange
  } |
  {
    tag: 'daterange'
    val: Daterange
  } |
  {
    tag: 'money'
    val: bigint
  } |
  {
    tag: 'oid'
    val: number
  } |
  {
    tag: 'enumeration'
    val: Enumeration
  } |
  {
    tag: 'composite'
    val: Composite
  } |
  {
    tag: 'domain'
    val: Domain
  } |
  {
    tag: 'array'
    val: LazyDbValue[]
  } |
  {
    tag: 'range'
    val: Range
  } |
  {
    tag: 'null'
  };
  export type CompositeType = {
    name: string;
    attributes: [string, LazyDbColumnType][];
  };
  export type DomainType = {
    name: string;
    baseType: LazyDbColumnType;
  };
  export type RangeType = {
    name: string;
    baseType: LazyDbColumnType;
  };
  export type DbColumnType = {
    tag: 'character'
  } |
  {
    tag: 'int2'
  } |
  {
    tag: 'int4'
  } |
  {
    tag: 'int8'
  } |
  {
    tag: 'float4'
  } |
  {
    tag: 'float8'
  } |
  {
    tag: 'numeric'
  } |
  {
    tag: 'boolean'
  } |
  {
    tag: 'text'
  } |
  {
    tag: 'varchar'
  } |
  {
    tag: 'bpchar'
  } |
  {
    tag: 'timestamp'
  } |
  {
    tag: 'timestamptz'
  } |
  {
    tag: 'date'
  } |
  {
    tag: 'time'
  } |
  {
    tag: 'timetz'
  } |
  {
    tag: 'interval'
  } |
  {
    tag: 'bytea'
  } |
  {
    tag: 'uuid'
  } |
  {
    tag: 'xml'
  } |
  {
    tag: 'json'
  } |
  {
    tag: 'jsonb'
  } |
  {
    tag: 'jsonpath'
  } |
  {
    tag: 'inet'
  } |
  {
    tag: 'cidr'
  } |
  {
    tag: 'macaddr'
  } |
  {
    tag: 'bit'
  } |
  {
    tag: 'varbit'
  } |
  {
    tag: 'int4range'
  } |
  {
    tag: 'int8range'
  } |
  {
    tag: 'numrange'
  } |
  {
    tag: 'tsrange'
  } |
  {
    tag: 'tstzrange'
  } |
  {
    tag: 'daterange'
  } |
  {
    tag: 'money'
  } |
  {
    tag: 'oid'
  } |
  {
    tag: 'enumeration'
    val: EnumerationType
  } |
  {
    tag: 'composite'
    val: CompositeType
  } |
  {
    tag: 'domain'
    val: DomainType
  } |
  {
    tag: 'array'
    val: LazyDbColumnType
  } |
  {
    tag: 'range'
    val: RangeType
  };
  export type DbColumn = {
    ordinal: bigint;
    name: string;
    dbType: DbColumnType;
    dbTypeName: string;
  };
  /**
   * A single row of values
   */
  export type DbRow = {
    values: DbValue[];
  };
  export type DbResult = {
    columns: DbColumn[];
    rows: DbRow[];
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
