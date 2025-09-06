declare module 'golem:agent/common' {
  import * as golemRpc022Types from 'golem:rpc/types@0.2.2';
  export type ValueAndType = golemRpc022Types.ValueAndType;
  export type WitType = golemRpc022Types.WitType;
  export type WitValue = golemRpc022Types.WitValue;
  export type Url = string;
  export type TextType = {
    languageCode: string;
  };
  export type TextSource = {
    data: string;
    textType: TextType | undefined;
  };
  export type TextReference = {
    tag: 'url'
    val: string
  } |
  {
    tag: 'inline'
    val: TextSource
  };
  export type TextDescriptor = {
    restrictions: TextType[] | undefined;
  };
  export type BinaryType = {
    mimeType: string;
  };
  export type BinaryDescriptor = {
    restrictions: BinaryType[] | undefined;
  };
  export type ElementSchema = {
    tag: 'component-model'
    val: WitType
  } |
  {
    tag: 'unstructured-text'
    val: TextDescriptor
  } |
  {
    tag: 'unstructured-binary'
    val: BinaryDescriptor
  };
  export type DataSchema = {
    tag: 'tuple'
    val: [string, ElementSchema][]
  } |
  {
    tag: 'multimodal'
    val: [string, ElementSchema][]
  };
  export type AgentMethod = {
    name: string;
    description: string;
    promptHint: string | undefined;
    inputSchema: DataSchema;
    outputSchema: DataSchema;
  };
  export type AgentConstructor = {
    name: string | undefined;
    description: string;
    promptHint: string | undefined;
    inputSchema: DataSchema;
  };
  export type AgentDependency = {
    typeName: string;
    description: string | undefined;
    constructor: AgentConstructor;
    methods: AgentMethod[];
  };
  export type AgentType = {
    typeName: string;
    description: string;
    constructor: AgentConstructor;
    methods: AgentMethod[];
    dependencies: AgentDependency[];
  };
  export type BinarySource = {
    data: Uint8Array;
    binaryType: BinaryType;
  };
  export type BinaryReference = {
    tag: 'url'
    val: Url
  } |
  {
    tag: 'inline'
    val: BinarySource
  };
  export type ElementValue = {
    tag: 'component-model'
    val: WitValue
  } |
  {
    tag: 'unstructured-text'
    val: TextReference
  } |
  {
    tag: 'unstructured-binary'
    val: BinaryReference
  };
  export type DataValue = {
    tag: 'tuple'
    val: ElementValue[]
  } |
  {
    tag: 'multimodal'
    val: [string, ElementValue][]
  };
  export type AgentError = {
    tag: 'invalid-input'
    val: string
  } |
  {
    tag: 'invalid-method'
    val: string
  } |
  {
    tag: 'invalid-type'
    val: string
  } |
  {
    tag: 'invalid-agent-id'
    val: string
  } |
  {
    tag: 'custom-error'
    val: ValueAndType
  };
}
