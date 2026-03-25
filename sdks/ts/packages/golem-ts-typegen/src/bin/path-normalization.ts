import path from 'path';

export function normalizeCliPath(inputPath: string, cwd: string = process.cwd()): string {
  if (path.isAbsolute(inputPath)) {
    return path.normalize(inputPath);
  }

  return path.normalize(path.resolve(cwd, inputPath));
}

export function normalizeFilePatterns(patterns: string[], cwd: string = process.cwd()): string[] {
  return patterns.map((pattern) => normalizeCliPath(pattern, cwd));
}
