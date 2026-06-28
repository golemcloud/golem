import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, URL } from 'node:url';

const packageRoot = path.resolve(fileURLToPath(new URL('..', import.meta.url)));
const exportsPath = path.join(packageRoot, 'types', 'exports.d.ts');

const lines = fs.readFileSync(exportsPath, 'utf8').split('\n');
const seenGuestTypeAliases = new Set();
let inGuestNamespace = false;

const patched = lines
  .map((line) =>
    line.startsWith('    export function invoke(toolName:')
      ? line.replace('export function invoke(', 'export function invokeTool(')
      : line,
  )
  .filter((line) => {
    if (line === '  export namespace guest {') {
      inGuestNamespace = true;
      return true;
    }

    if (inGuestNamespace && line === '  }') {
      inGuestNamespace = false;
      return true;
    }

    const alias = inGuestNamespace
      ? line.match(/^    export type (\w+)(?:<[^>]+>)? = /)?.[1]
      : undefined;
    if (alias) {
      if (seenGuestTypeAliases.has(alias)) return false;
      seenGuestTypeAliases.add(alias);
    }

    return true;
  });

const guestNamespace = [
  '  export namespace guest {',
  '    export function initialize(agentType: string, input: SchemaValueTree, principal: Principal): Promise<void>;',
  '    export function invoke(methodName: string, input: SchemaValueTree, principal: Principal): Promise<SchemaValueTree | undefined>;',
  '    export function getDefinition(): Promise<AgentType>;',
  '    export function discoverAgentTypes(): Promise<AgentType[]>;',
  '    export function discoverTools(): Promise<Tool[]>;',
  '    export function getTool(name: string): Promise<Tool>;',
  '    export function invokeTool(toolName: string, commandPath: string[], input: TypedSchemaValue, stdin: InputStream | undefined, principal: Principal): Promise<InvocationResult>;',
  '    export type SchemaValueTree = golemAgent200Guest.SchemaValueTree;',
  '    export type AgentError = golemAgent200Guest.AgentError;',
  '    export type AgentType = golemAgent200Guest.AgentType;',
  '    export type Principal = golemAgent200Guest.Principal;',
  '    export type Tool = golemTool010Guest.Tool;',
  '    export type ToolError = golemTool010Guest.ToolError;',
  '    export type InvocationResult = golemTool010Guest.InvocationResult;',
  '    export type TypedSchemaValue = golemTool010Guest.TypedSchemaValue;',
  '    export type InputStream = golemTool010Guest.InputStream;',
  '  }',
];

const moduleEnd = patched.lastIndexOf('}');
if (moduleEnd === -1) {
  throw new Error(`Could not find the end of the agent-guest declaration in ${exportsPath}.`);
}

patched.splice(moduleEnd, 0, ...guestNamespace);

fs.writeFileSync(exportsPath, patched.join('\n'), 'utf8');
