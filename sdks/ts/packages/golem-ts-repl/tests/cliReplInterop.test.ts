import { describe, expect, it } from 'vitest';
import { parseRawArgs } from '../src/cli-repl-interop';

describe('parseRawArgs', () => {
  it('keeps component/agent ids with quoted params as a single token', () => {
    expect(parseRawArgs('app:ts-main/CounterAgent("??")')).toEqual([
      'app:ts-main/CounterAgent("??")',
    ]);
  });

  it('keeps spaces inside quoted params for component/agent ids', () => {
    expect(parseRawArgs('app:ts-main/CounterAgent("a b") --help')).toEqual([
      'app:ts-main/CounterAgent("a b")',
      '--help',
    ]);
  });

  it('keeps agent ids with quoted params as a single token', () => {
    expect(parseRawArgs('app:ts-main/CounterAgent("??")')).toEqual([
      'app:ts-main/CounterAgent("??")',
    ]);
  });

  it('keeps spaces inside quoted params for agent ids', () => {
    expect(parseRawArgs('app:ts-main/CounterAgent("a b") --help')).toEqual([
      'app:ts-main/CounterAgent("a b")',
      '--help',
    ]);
  });
});
