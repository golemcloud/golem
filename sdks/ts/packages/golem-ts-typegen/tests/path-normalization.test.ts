import path from 'path';
import { describe, expect, it } from 'vitest';
import { normalizeCliPath, normalizeFilePatterns } from '../src/bin/path-normalization.js';

describe('path normalization', () => {
  it('normalizes dotted absolute paths', () => {
    const input = '/tmp/test-components/agent-promise/./src/**/*.ts';
    const expected = path.normalize('/tmp/test-components/agent-promise/src/**/*.ts');

    expect(normalizeCliPath(input)).toBe(expected);
  });

  it('resolves and normalizes dotted relative paths', () => {
    const cwd = '/tmp/work';
    const input = './src/../src/**/*.ts';
    const expected = path.normalize(path.resolve(cwd, 'src/**/*.ts'));

    expect(normalizeCliPath(input, cwd)).toBe(expected);
  });

  it('normalizes all file patterns', () => {
    const cwd = '/tmp/work';
    const inputs = ['./src/**/*.ts', '/tmp/work/./other/**/*.ts'];
    const expected = [
      path.normalize(path.resolve(cwd, 'src/**/*.ts')),
      path.normalize('/tmp/work/other/**/*.ts'),
    ];

    expect(normalizeFilePatterns(inputs, cwd)).toEqual(expected);
  });
});
