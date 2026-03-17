import { describe, expect, it } from 'vitest';
import { parseRawArgs } from '../src/cli-repl-interop';

describe('parseRawArgs', () => {
  it('keeps agent ids with quoted params as a single token', () => {
    expect(parseRawArgs('CounterAgent("??")')).toEqual(['CounterAgent("??")']);
  });

  it('keeps spaces inside quoted params for agent ids', () => {
    expect(parseRawArgs('CounterAgent("a b") --help')).toEqual(['CounterAgent("a b")', '--help']);
  });
});
