export type AnalysedType =
  | AnalysedType_TypeVariant
  | AnalysedType_TypeResult
  | AnalysedType_TypeOption
  | AnalysedType_TypeEnum
  | AnalysedType_TypeFlags
  | AnalysedType_TypeRecord
  | AnalysedType_TypeTuple
  | AnalysedType_TypeList
  | AnalysedType_TypeStr
  | AnalysedType_TypeChr
  | AnalysedType_TypeF64
  | AnalysedType_TypeF32
  | AnalysedType_TypeU64
  | AnalysedType_TypeS64
  | AnalysedType_TypeU32
  | AnalysedType_TypeS32
  | AnalysedType_TypeU16
  | AnalysedType_TypeS16
  | AnalysedType_TypeU8
  | AnalysedType_TypeS8
  | AnalysedType_TypeBool
  | AnalysedType_TypeHandle;

export interface TypeOption {
  inner: AnalysedType;
}

export interface NameOptionTypePair {
  name: string;
  typ: AnalysedType | null; // Assuming "Option" implies it can be nullable
}

export interface NameTypePair {
  name: string;
  typ: AnalysedType;
}

export interface TypeVariant {
  cases: NameOptionTypePair[];
}

export interface TypeList {
  inner: AnalysedType;
}

export interface TypeResult {
  ok: AnalysedType; // Represents the 'ok' analyzed type
  err: AnalysedType; // Represents the 'err' analyzed type
}

export interface AnalysedType_Base {
  type: string;
}

export interface AnalysedType_TypeBool extends AnalysedType_Base {
  type: "Bool";
}

export interface AnalysedType_TypeChr extends AnalysedType_Base {
  type: "Chr";
}

export interface AnalysedType_TypeEnum extends AnalysedType_Base {
  type: "Enum";
  cases: string[];
}

export interface AnalysedType_TypeF32 extends AnalysedType_Base {
  type: "F32";
}

export interface AnalysedType_TypeF64 extends AnalysedType_Base {
  type: "F64";
}

export interface AnalysedType_TypeFlags extends AnalysedType_Base {
  type: "Flags";
  names: string[];
}

export interface AnalysedType_TypeHandle extends AnalysedType_Base {
  type: "Handle";
  resource_id: number;
  mode: AnalysedResourceMode;
}

export interface AnalysedType_TypeList extends AnalysedType_Base {
  type: "List";
  inner: AnalysedType;
}
export interface AnalysedType_TypeOption extends AnalysedType_Base {
  type: "Option"; // Fixed enum value
  inner: AnalysedType; // Comes from TypeOption
}

interface AnalysedType_TypeRecord extends AnalysedType_Base {
  type: "Record";
  fields: NameTypePair[];
}

export interface AnalysedType_TypeResult extends AnalysedType_Base {
  type: "Result"; // Fixed enum value
  ok: AnalysedType; // Comes from TypeResult
  err: AnalysedType; // Comes from TypeResult
}

export interface AnalysedType_TypeS16 extends AnalysedType_Base {
  type: "S16";
}

export interface AnalysedType_TypeS32 extends AnalysedType_Base {
  type: "S32";
}

export interface AnalysedType_TypeS64 extends AnalysedType_Base {
  type: "S64";
}

export interface AnalysedType_TypeS8 extends AnalysedType_Base {
  type: "S8";
}

export interface AnalysedType_TypeStr extends AnalysedType_Base {
  type: "Str";
}

export interface AnalysedType_TypeTuple extends AnalysedType_Base {
  type: "Tuple";
}

export interface AnalysedType_TypeU16 extends AnalysedType_Base {
  type: "U16";
}

export interface AnalysedType_TypeU32 extends AnalysedType_Base {
  type: "U32";
}

export interface AnalysedType_TypeU64 extends AnalysedType_Base {
  type: "U64";
}

export interface AnalysedType_TypeU8 extends AnalysedType_Base {
  type: "U8";
}

export interface AnalysedType_TypeVariant extends AnalysedType_Base {
  type: "Variant";
  cases: NameOptionTypePair[];
}

export interface TimestampParameter {
  timestamp: string; // ISO date-time format
}

export interface TypeAnnotatedValue {
  typ: AnalysedType;
  value: unknown;
}

export interface TypeEnum {
  cases: string[];
}

export interface TypeFlags {
  names: string[];
}

export interface TypeHandle {
  resource_id: number; // uint64
  mode: AnalysedResourceMode;
}
// Define AnalysedResourceMode if it is required elsewhere in the schema
export type AnalysedResourceMode = string; // Replace with exact type if known
