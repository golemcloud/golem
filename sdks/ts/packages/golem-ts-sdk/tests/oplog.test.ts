import { describe, expect, it, vi } from 'vitest';
import {
  enrichOplogEntries as rawEnrichOplogEntries,
  type PublicOplogEntry as RawPublicOplogEntry,
} from 'golem:api/oplog@1.5.0';
import { AgentId, enrichOplogEntries } from '../src/host/oplog';

describe('oplog checkpoint enrichment', () => {
  it('preserves the source component revision while wrapping the target agent ID', () => {
    const environmentId = { uuid: { highBits: 0n, lowBits: 1n } };
    const agentId = { uuid: { highBits: 0x00000007_0000_8000n, lowBits: 0x8000_0000_0000_0002n } };
    const checkpoint: RawPublicOplogEntry = {
      tag: 'oplog-processor-checkpoint',
      val: {
        timestamp: { seconds: 1700000000n, nanoseconds: 0 },
        plugin: {
          environmentPluginGrantId: { uuid: { highBits: 0n, lowBits: 3n } },
          pluginPriority: 0,
          pluginName: 'processor',
          pluginVersion: '1.0',
          parameters: [],
        },
        sourceComponentRevision: 9007199254740993n,
        targetAgentId: agentId,
        confirmedUpTo: 11n,
        sendingUpTo: 15n,
        lastBatchStart: 12n,
      },
    };
    vi.mocked(rawEnrichOplogEntries).mockReturnValue([checkpoint]);

    const [entry] = enrichOplogEntries(environmentId, agentId, [], 99n);
    expect(entry?.tag).toBe('oplog-processor-checkpoint');
    if (entry?.tag !== 'oplog-processor-checkpoint') throw new Error('Expected checkpoint');
    expect(entry.val.sourceComponentRevision).toBe(9007199254740993n);
    expect(entry.val.targetAgentId).toBeInstanceOf(AgentId);
    expect(entry.val.targetAgentId.toString()).toBe('00000007-0000-8000-8000-000000000002');
    expect(entry.val.confirmedUpTo).toBe(11n);
    expect(entry.val.sendingUpTo).toBe(15n);
    expect(entry.val.lastBatchStart).toBe(12n);
  });

  it('keeps the never-live mark of a create entry', () => {
    const environmentId = { uuid: { highBits: 0n, lowBits: 1n } };
    const agentId = { uuid: { highBits: 0x00000007_0000_8000n, lowBits: 0x8000_0000_0000_0002n } };
    const create = (neverLive: boolean): RawPublicOplogEntry => ({
      tag: 'create',
      val: {
        timestamp: { seconds: 1700000000n, nanoseconds: 0 },
        agentId,
        ownerKind: 'component-agent',
        agentMode: 'durable',
        componentRevision: 3n,
        env: [],
        createdBy: { uuid: { highBits: 0n, lowBits: 4n } },
        environmentId,
        componentSize: 1024n,
        initialTotalLinearMemorySize: 65536n,
        initialActivePlugins: [],
        localAgentConfig: [],
        instanceId: { highBits: 0n, lowBits: 5n },
        neverLive,
      },
    });
    vi.mocked(rawEnrichOplogEntries).mockReturnValue([create(true), create(false)]);

    const [marked, unmarked] = enrichOplogEntries(environmentId, agentId, [], 99n);
    if (marked?.tag !== 'create' || unmarked?.tag !== 'create') {
      throw new Error('Expected create entries');
    }
    expect(marked.val.neverLive).toBe(true);
    expect(unmarked.val.neverLive).toBe(false);
    expect(marked.val.agentId.toString()).toBe('00000007-0000-8000-8000-000000000002');
  });
});
