import { describe, expect, it, vi } from 'vitest';
import {
  enrichOplogEntries as rawEnrichOplogEntries,
  type PublicOplogEntry as RawPublicOplogEntry,
} from 'golem:api/oplog@1.5.0';
import { enrichOplogEntries } from '../src/host/oplog';

describe('oplog checkpoint enrichment', () => {
  it('preserves the source component revision while wrapping the target agent ID', () => {
    const environmentId = { uuid: { highBits: 0n, lowBits: 1n } };
    const agentId = {
      componentId: { uuid: { highBits: 0n, lowBits: 2n } },
      agentId: 'processor',
    };
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
    expect(entry.val.targetAgentId.agentId).toBe('processor');
    expect(entry.val.confirmedUpTo).toBe(11n);
    expect(entry.val.sendingUpTo).toBe(15n);
    expect(entry.val.lastBatchStart).toBe(12n);
  });
});
