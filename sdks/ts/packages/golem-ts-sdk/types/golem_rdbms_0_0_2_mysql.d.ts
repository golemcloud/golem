declare module 'golem:rdbms/mysql@0.0.2' {
  import * as golemRdbms002Types from 'golem:rdbms/types@0.0.2';
  export class DbResultStream {
    getColumns(): DbColumn[];
    getNext(): DbRow[] | undefined;
  }
  export class DbConnection {
    /**
     * @throws Error
     */
    static open(address: string): DbConnection;
    /**
     * @throws Error
     */
    query(statement: string, params: DbValue[]): DbResult;
    /**
     * @throws Error
     */
    queryStream(statement: string, params: DbValue[]): DbResultStream;
    /**
     * @throws Error
     */
    execute(statement: string, params: DbValue[]): bigint;
    /**
     * @throws Error
     */
    beginTransaction(): DbTransaction;
  }
  export class DbTransaction {
    /**
     * @throws Error
     */
    query(statement: string, params: DbValue[]): DbResult;
    /**
     * @throws Error
     */
    queryStream(statement: string, params: DbValue[]): DbResultStream;
    /**
     * @throws Error
     */
    execute(statement: string, params: DbValue[]): bigint;
    /**
     * @throws Error
     */
    commit(): void;
    /**
     * @throws Error
     */
    rollback(): void;
  }
  export type Date = golemRdbms002Types.Date;
  export type Time = golemRdbms002Types.Time;
  export type Timestamp = golemRdbms002Types.Timestamp;
  export type Error = 
  {
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
  export type DbColumnType = 
  {
    tag: 'boolean'
  } |
  {
    tag: 'tinyint'
  } |
  {
    tag: 'smallint'
  } |
  {
    tag: 'mediumint'
  } |
  {
    tag: 'int'
  } |
  {
    tag: 'bigint'
  } |
  {
    tag: 'tinyint-unsigned'
  } |
  {
    tag: 'smallint-unsigned'
  } |
  {
    tag: 'mediumint-unsigned'
  } |
  {
    tag: 'int-unsigned'
  } |
  {
    tag: 'bigint-unsigned'
  } |
  {
    tag: 'float'
  } |
  {
    tag: 'double'
  } |
  {
    tag: 'decimal'
  } |
  {
    tag: 'date'
  } |
  {
    tag: 'datetime'
  } |
  {
    tag: 'timestamp'
  } |
  {
    tag: 'time'
  } |
  {
    tag: 'year'
  } |
  {
    tag: 'fixchar'
  } |
  {
    tag: 'varchar'
  } |
  {
    tag: 'tinytext'
  } |
  {
    tag: 'text'
  } |
  {
    tag: 'mediumtext'
  } |
  {
    tag: 'longtext'
  } |
  {
    tag: 'binary'
  } |
  {
    tag: 'varbinary'
  } |
  {
    tag: 'tinyblob'
  } |
  {
    tag: 'blob'
  } |
  {
    tag: 'mediumblob'
  } |
  {
    tag: 'longblob'
  } |
  {
    tag: 'enumeration'
  } |
  {
    tag: 'set'
  } |
  {
    tag: 'bit'
  } |
  {
    tag: 'json'
  };
  export type DbColumn = {
    ordinal: bigint;
    name: string;
    dbType: DbColumnType;
    dbTypeName: string;
  };
  /**
   * Value descriptor for a single database value
   */
  export type DbValue = 
  {
    tag: 'boolean'
    val: boolean
  } |
  {
    tag: 'tinyint'
    val: number
  } |
  {
    tag: 'smallint'
    val: number
  } |
  {
    tag: 'mediumint'
    val: number
  } |
  {
    tag: 'int'
    val: number
  } |
  {
    tag: 'bigint'
    val: bigint
  } |
  {
    tag: 'tinyint-unsigned'
    val: number
  } |
  {
    tag: 'smallint-unsigned'
    val: number
  } |
  {
    tag: 'mediumint-unsigned'
    val: number
  } |
  {
    tag: 'int-unsigned'
    val: number
  } |
  {
    tag: 'bigint-unsigned'
    val: bigint
  } |
  {
    tag: 'float'
    val: number
  } |
  {
    tag: 'double'
    val: number
  } |
  {
    tag: 'decimal'
    val: string
  } |
  {
    tag: 'date'
    val: Date
  } |
  {
    tag: 'datetime'
    val: Timestamp
  } |
  {
    tag: 'timestamp'
    val: Timestamp
  } |
  {
    tag: 'time'
    val: Time
  } |
  {
    tag: 'year'
    val: number
  } |
  {
    tag: 'fixchar'
    val: string
  } |
  {
    tag: 'varchar'
    val: string
  } |
  {
    tag: 'tinytext'
    val: string
  } |
  {
    tag: 'text'
    val: string
  } |
  {
    tag: 'mediumtext'
    val: string
  } |
  {
    tag: 'longtext'
    val: string
  } |
  {
    tag: 'binary'
    val: Uint8Array
  } |
  {
    tag: 'varbinary'
    val: Uint8Array
  } |
  {
    tag: 'tinyblob'
    val: Uint8Array
  } |
  {
    tag: 'blob'
    val: Uint8Array
  } |
  {
    tag: 'mediumblob'
    val: Uint8Array
  } |
  {
    tag: 'longblob'
    val: Uint8Array
  } |
  {
    tag: 'enumeration'
    val: string
  } |
  {
    tag: 'set'
    val: string
  } |
  {
    tag: 'bit'
    val: boolean[]
  } |
  {
    tag: 'json'
    val: string
  } |
  {
    tag: 'null'
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
