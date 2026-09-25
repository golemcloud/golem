import type { Tool, TypedSchemaValue } from 'golem:tool/common@0.1.0';
import {
  SchemaValueStream,
  type SchemaValueNode,
  type SchemaValueTree,
} from 'golem:core/types@2.0.0';
import { bindConcreteToolCommand } from '../../tool';
import { wireResources } from './wireResources';
import {
  prepareAgentStream,
  prepareAgentStreamLift,
  type AgentStream,
} from '../../schema/agentStream';
import { GuestSchemaValueStreamHandle } from '../schema-model/schemaValueStreamHandle';

export interface ConcreteCodec {
  read(reader: WireReader, index: number): unknown;
  write(value: unknown, writer: WireWriter): number;
}

export interface TypedCodec {
  codec: ConcreteCodec;
  graph: TypedSchemaValue['graph'];
}

export interface CompiledCommand {
  path: string[];
  aliases: string[][];
  nested: boolean;
  input: TypedCodec;
  result?: TypedCodec;
  errors: Record<string, TypedCodec | undefined>;
  stdin?: { required: boolean };
  stdout?: { required: boolean };
}

class WireReader {
  private readonly visited = new Set<number>();
  private readonly resources = new Set<object>();
  private readonly ownedNodes: Array<{ tag: string; val: unknown }> = [];
  private readonly streams: Array<{ node: { val: unknown }; commit(): void }> = [];
  constructor(readonly tree: SchemaValueTree) {}

  node(index: number, tag: SchemaValueNode['tag'], decode: (node: SchemaValueNode) => unknown) {
    if (!Number.isInteger(index) || index < 0 || index >= this.tree.valueNodes.length)
      throw new TypeError(`invalid wire index ${index}`);
    if (this.visited.has(index)) throw new TypeError(`aliased wire index ${index}`);
    const node = this.tree.valueNodes[index];
    if (node.tag !== tag) throw new TypeError(`expected ${tag}, received ${node.tag}`);
    this.visited.add(index);
    return decode(node);
  }

  resource(node: { tag: string; val: unknown }): object {
    const raw = node.val;
    if (raw === null || typeof raw !== 'object' || this.resources.has(raw))
      throw new TypeError('invalid or aliased resource');
    wireResources[node.tag]!.check(raw, node);
    this.resources.add(raw);
    this.ownedNodes.push(node);
    return raw;
  }

  stream(node: { val: SchemaValueStream }, codec: ConcreteCodec): AgentStream<unknown> {
    if (!node.val || typeof node.val !== 'object' || this.resources.has(node.val))
      throw new TypeError('invalid or aliased stream');
    this.resources.add(node.val);
    const prepared = prepareAgentStreamLift({ kind: 'wrapped', value: node.val }, (tree) =>
      readConcreteAsync(codec, tree),
    );
    this.streams.push({ node, commit: prepared.commit });
    return prepared.stream;
  }

  finish() {
    if (this.visited.size !== this.tree.valueNodes.length)
      throw new TypeError('unreachable wire nodes');
    for (const node of this.ownedNodes) {
      wireResources[node.tag]!.lift(node.val as object, node);
      node.val = undefined;
    }
    for (const stream of this.streams) {
      stream.node.val = undefined;
      stream.commit();
    }
  }
}

class WireWriter {
  readonly valueNodes: SchemaValueNode[] = [];
  private readonly resources: Array<{
    node: object;
    commit(node: object): void;
    rollback(): void;
  }> = [];
  private readonly streams: Array<
    ReturnType<typeof prepareAgentStream> & {
      node: Extract<SchemaValueNode, { tag: 'stream-value' }>;
    }
  > = [];

  stream(value: AgentStream<unknown>, codec: ConcreteCodec): number {
    const prepared = prepareAgentStream(value, (item) => writeConcreteAsync(codec, item));
    const node = { tag: 'stream-value', val: undefined } as unknown as Extract<
      SchemaValueNode,
      { tag: 'stream-value' }
    >;
    this.streams.push({ ...prepared, node });
    return this.add(node);
  }

  resource(tag: SchemaValueNode['tag'], raw: unknown): number {
    if (raw === null || typeof raw !== 'object') throw new TypeError('invalid resource');
    const node = { tag, val: raw } as SchemaValueNode;
    const move = wireResources[tag]!.adopt(raw);
    this.resources.push({ node, ...move });
    return this.add(node);
  }

  finish(): void {
    if (this.streams.some((stream) => stream.endpoint.kind !== 'wrapped'))
      throw new TypeError('native schema streams require asynchronous encoding');
    for (const stream of this.streams)
      if (stream.endpoint.kind === 'wrapped') stream.node.val = stream.endpoint.value;
    for (const resource of this.resources) resource.commit(resource.node);
  }

  async finishAsync(): Promise<void> {
    try {
      for (const stream of this.streams) {
        if (stream.endpoint.kind === 'native') {
          const value = await SchemaValueStream.wrap(stream.endpoint.value);
          stream.endpoint = { kind: 'wrapped', value };
        }
      }
      this.finish();
    } catch (error) {
      for (const stream of this.streams.splice(0)) {
        try {
          await new GuestSchemaValueStreamHandle(stream.endpoint).close();
        } catch {
          /* preserve the encode failure */
        }
      }
      throw error;
    }
  }

  rollback(): void {
    for (const resource of this.resources.slice().reverse()) resource.rollback();
    for (const stream of this.streams.slice().reverse()) stream.rollback();
  }

  trial(write: () => number): number | undefined {
    const nodes = this.valueNodes.length;
    const resources = this.resources.length;
    const streams = this.streams.length;
    try {
      return write();
    } catch {
      for (const resource of this.resources.splice(resources).reverse()) resource.rollback();
      for (const stream of this.streams.splice(streams).reverse()) stream.rollback();
      this.valueNodes.length = nodes;
      return undefined;
    }
  }

  add(node: SchemaValueNode): number {
    return this.valueNodes.push(node) - 1;
  }
}

function discard(tree: SchemaValueTree): Promise<unknown> {
  const dropped = new Set<unknown>();
  const streams: Promise<void>[] = [];
  for (const node of tree.valueNodes) {
    if (
      ['secret-value', 'quota-token-handle', 'permission-card-handle', 'stream-value'].includes(
        node.tag,
      )
    ) {
      const resource = (node as { val: object | undefined }).val;
      (node as { val: unknown }).val = undefined;
      if (resource === undefined || dropped.has(resource)) continue;
      dropped.add(resource);
      try {
        if (node.tag === 'stream-value') {
          streams.push(
            new GuestSchemaValueStreamHandle({
              kind: 'wrapped',
              value: resource as SchemaValueStream,
            })
              .close()
              .catch(() => {}),
          );
          continue;
        }
        const ops = wireResources[node.tag];
        if (ops) ops.lift(resource, node);
        const dispose = (resource as { [Symbol.dispose]?: () => void })[Symbol.dispose];
        dispose?.call(resource);
      } catch {
        // Attempt every drop without replacing the decoding failure.
      }
    }
  }
  return Promise.all(streams);
}

export function encodeToolValue(
  codec: TypedCodec,
  value: unknown,
  position: string,
): TypedSchemaValue {
  try {
    return { graph: codec.graph, value: writeConcrete(codec.codec, value) };
  } catch (error) {
    throw invalidToolResult(`${position}: ${String(error)}`);
  }
}

export function writeConcrete(codec: ConcreteCodec, value: unknown): SchemaValueTree {
  const writer = new WireWriter();
  try {
    const root = codec.write(value, writer);
    writer.finish();
    return { valueNodes: writer.valueNodes, root };
  } catch (error) {
    // No resource has crossed the wire boundary; the caller still owns them.
    writer.rollback();
    throw error;
  }
}

export async function writeConcreteAsync(
  codec: ConcreteCodec,
  value: unknown,
): Promise<SchemaValueTree> {
  const writer = new WireWriter();
  try {
    const root = codec.write(value, writer);
    await writer.finishAsync();
    return { valueNodes: writer.valueNodes, root };
  } catch (error) {
    writer.rollback();
    throw error;
  }
}

export async function readConcreteAsync(
  codec: ConcreteCodec,
  tree: SchemaValueTree,
): Promise<unknown> {
  const reader = new WireReader(tree);
  try {
    const result = codec.read(reader, tree.root);
    reader.finish();
    return result;
  } catch (error) {
    await discard(tree);
    throw error;
  }
}

export function readConcrete(codec: ConcreteCodec, tree: SchemaValueTree): unknown {
  const reader = new WireReader(tree);
  try {
    const result = codec.read(reader, tree.root);
    reader.finish();
    return result;
  } catch (error) {
    void discard(tree);
    throw error;
  }
}

export const invalidToolResult = (val: string) => ({ tag: 'invalid-result' as const, val });

interface DeclaredError {
  tag: 'err';
  name: string;
  hasPayload: boolean;
  payload?: unknown;
}

export function isDeclaredToolError(value: unknown): value is DeclaredError {
  return (
    typeof value === 'object' &&
    value !== null &&
    (value as DeclaredError).tag === 'err' &&
    typeof (value as DeclaredError).name === 'string' &&
    typeof (value as DeclaredError).hasPayload === 'boolean'
  );
}

export function encodeDeclaredToolErrorPayload(
  errorCase: { payloadCodec?: TypedCodec },
  error: DeclaredError,
  position: string,
): TypedSchemaValue {
  if (
    error.hasPayload !== (errorCase.payloadCodec !== undefined) ||
    Object.prototype.hasOwnProperty.call(error, 'payload') !== error.hasPayload
  )
    throw invalidToolResult(`${position}: unexpected payload presence`);
  if (errorCase.payloadCodec)
    return encodeToolValue(errorCase.payloadCodec, error.payload, position);
  return {
    graph: {
      defs: [],
      typeNodes: [
        { body: { tag: 'tuple-type', val: [] }, metadata: { aliases: [], examples: [] } },
      ],
      root: 0,
    },
    value: { valueNodes: [{ tag: 'tuple-value', val: [] }], root: 0 },
  };
}

type Handler = (input: unknown, context: unknown) => unknown;
interface Entry {
  descriptor: Tool;
  commands: Array<CompiledCommand & { handler: Handler; receiver: object }>;
}
const registry = new Map<string, Entry>();
const failures: Array<{ toolName: string; messages: string[] }> = [];

/** Entry point for compiler-emitted descriptors, codecs, and command bindings. */
export function compiledTool(name: string, descriptor: Tool, commands: CompiledCommand[]) {
  return {
    implement(implementation: object) {
      try {
        if (registry.has(name)) throw new Error(`Tool "${name}" is already registered`);
        const bindings = commands.map((command) => {
          const { handler, receiver } = bindConcreteToolCommand(
            implementation,
            name,
            command.path,
            command.nested,
          );
          return { ...command, handler, receiver };
        });
        registry.set(name, { descriptor, commands: bindings });
      } catch (error) {
        failures.push({ toolName: name, messages: [String(error)] });
      }
      return { name };
    },
  };
}

export const ToolRegistry = {
  getRegistrationErrors: () => failures,
  getRegisteredTools: () =>
    [...registry].sort(([a], [b]) => a.localeCompare(b)).map(([, e]) => e.descriptor),
  getTool: (name: string) => registry.get(name)?.descriptor,
  resolveInvocation(name: string, path: string[]) {
    const entry = registry.get(name);
    if (!entry) throw { tag: 'invalid-tool-name', val: name };
    const command = entry.commands.find((command) =>
      command.aliases.some(
        (p) => p.length === path.length && p.every((segment, i) => segment === path[i]),
      ),
    );
    if (!command) throw { tag: 'invalid-command-path', val: path };
    return {
      command: {
        body: {
          result: command.result ? { codec: command.result } : undefined,
          errors: Object.entries(command.errors).map(([name, payloadCodec]) => ({
            name,
            payloadCodec,
          })),
          stdin: command.stdin,
          stdout: command.stdout,
        },
      },
      prepareWire(input: TypedSchemaValue) {
        const args = readConcrete(command.input.codec, input.value);
        return {
          invoke: (context: unknown) => command.handler.call(command.receiver, args, context),
        };
      },
    };
  },
};
