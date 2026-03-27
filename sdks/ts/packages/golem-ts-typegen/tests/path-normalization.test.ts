import path from 'path';
import { describe, expect, it } from 'vitest';
import { normalizeCliPath, normalizeFilePatterns } from '../src/bin/path-normalization.js';

function toForwardSlashPath(inputPath: string): string {
  return inputPath.split(path.sep).join('/');
}

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
      toForwardSlashPath(path.normalize(path.resolve(cwd, 'src/**/*.ts'))),
      toForwardSlashPath(path.normalize('/tmp/work/other/**/*.ts')),
    ];

    expect(normalizeFilePatterns(inputs, cwd)).toEqual(expected);
  });

  it('normalizes windows-style glob patterns to forward slashes', () => {
    const cwd = 'C:\\workspace\\golem-test\\test-app';
    const inputs = ['.\\src\\**\\*.ts', 'C:\\workspace\\golem-test\\test-app\\other\\**\\*.ts'];
    const expected = [
      'C:/workspace/golem-test/test-app/src/**/*.ts',
      'C:/workspace/golem-test/test-app/other/**/*.ts',
    ];

    expect(normalizeFilePatterns(inputs, cwd)).toEqual(expected);
  });
});
