import { describe, expect, it } from 'vitest';
import { agentIdsFromAgentListJson, parseRawArgs } from '../src/cli-repl-interop';

describe('parseRawArgs', () => {
  it('keeps agent ids with quoted params as a single token', () => {
    expect(parseRawArgs('CounterAgent("??")')).toEqual(['CounterAgent("??")']);
  });

  it('keeps spaces inside quoted params for agent ids', () => {
    expect(parseRawArgs('CounterAgent("a b") --help')).toEqual(['CounterAgent("a b")', '--help']);
  });
});

describe('agentIdsFromAgentListJson', () => {
  it('returns the agent ids of the agent list output', () => {
    expect(
      agentIdsFromAgentListJson({
        $type: 'agent.list',
        agents: [
          { componentName: 'app:counter', agentId: 'CounterAgent("a")' },
          { componentName: 'app:counter', agentId: 'CounterAgent("b")' },
        ],
        cursors: {},
      }),
    ).toEqual(['CounterAgent("a")', 'CounterAgent("b")']);
  });

  it('returns no agent ids for other output types', () => {
    expect(agentIdsFromAgentListJson({ $type: 'component.list', components: [] })).toEqual([]);
    expect(agentIdsFromAgentListJson(undefined)).toEqual([]);
  });
});
