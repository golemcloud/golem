import { describe, expect, it, vi } from 'vitest';
import { connectWebsocket, WebsocketError } from '../src/websocket';

const host = vi.hoisted(() => {
  vi.resetModules();
  return { connect: vi.fn() };
});
vi.mock('golem:websocket/client@1.5.0', () => ({ WebsocketConnection: host }));

describe('websocket reconstruction', () => {
  it('forwards the policy and normalizes absence to undefined', async () => {
    host.connect.mockReturnValue({});
    await connectWebsocket('wss://test');
    expect(host.connect).toHaveBeenLastCalledWith('wss://test', undefined, undefined);
    for (const reconstructionPolicy of [
      'reconnect-automatically',
      'report-connection-loss',
    ] as const) {
      await connectWebsocket('wss://test', { headers: [['a', 'b']], reconstructionPolicy });
      expect(host.connect).toHaveBeenLastCalledWith(
        'wss://test',
        [['a', 'b']],
        reconstructionPolicy,
      );
    }
  });

  it('preserves payloadless loss from every operation', async () => {
    const loss = { tag: 'session-lost' };
    host.connect.mockReturnValue({
      send() {
        throw loss;
      },
      receive() {
        return Promise.reject(loss);
      },
      receiveWithTimeout() {
        return Promise.reject(loss);
      },
      close() {
        throw loss;
      },
    });
    const socket = await connectWebsocket('wss://test');
    expect(() => socket.send('hello')).toThrow(WebsocketError);
    expect(() => socket.close()).toThrow(
      expect.objectContaining({ tag: 'session-lost', cause: loss }),
    );
    await expect(socket.receive()).rejects.toMatchObject({ tag: 'session-lost', cause: loss });
    await expect(socket.receiveWithTimeout(1)).rejects.toMatchObject({
      tag: 'session-lost',
      cause: loss,
    });
    expect(new WebsocketError({ tag: 'not-a-host-tag' }, 'send').tag).toBeUndefined();
    expect(new WebsocketError(new Error('host', { cause: loss }), 'send').tag).toBe('session-lost');
  });
});
