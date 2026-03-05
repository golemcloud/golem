import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import * as fs from 'node:fs/promises';
import * as os from 'node:os';
import * as path from 'node:path';
import { ScenarioLoader } from '../src/executor.js';

async function writeTempYaml(content: string): Promise<string> {
  const tmpDir = await fs.mkdtemp(path.join(os.tmpdir(), 'loader-test-'));
  const filePath = path.join(tmpDir, 'scenario.yaml');
  await fs.writeFile(filePath, content, 'utf8');
  return filePath;
}

describe('ScenarioLoader', () => {
  it('loads a valid scenario', async () => {
    const filePath = await writeTempYaml(`
name: "test-scenario"
steps:
  - id: "step-1"
    prompt: "Do something"
    expectedSkills:
      - "some-skill"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.name, 'test-scenario');
    assert.equal(spec.steps.length, 1);
    assert.equal(spec.steps[0].id, 'step-1');
    assert.deepEqual(spec.steps[0].expectedSkills, ['some-skill']);
  });

  it('loads a minimal valid scenario', async () => {
    const filePath = await writeTempYaml(`
name: "minimal"
steps:
  - prompt: "hello"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.name, 'minimal');
    assert.equal(spec.steps.length, 1);
  });

  it('rejects scenario without name', async () => {
    const filePath = await writeTempYaml(`
steps:
  - prompt: "hello"
`);
    await assert.rejects(
      () => ScenarioLoader.load(filePath),
      (err: Error) => {
        assert.ok(err.message.includes('Invalid scenario file'));
        return true;
      }
    );
  });

  it('rejects scenario without steps', async () => {
    const filePath = await writeTempYaml(`
name: "no-steps"
`);
    await assert.rejects(
      () => ScenarioLoader.load(filePath),
      (err: Error) => {
        assert.ok(err.message.includes('Invalid scenario file'));
        return true;
      }
    );
  });

  it('rejects scenario with empty steps array', async () => {
    const filePath = await writeTempYaml(`
name: "empty-steps"
steps: []
`);
    await assert.rejects(
      () => ScenarioLoader.load(filePath),
      (err: Error) => {
        assert.ok(err.message.includes('at least one step'));
        return true;
      }
    );
  });

  it('rejects scenario with invalid step field types', async () => {
    const filePath = await writeTempYaml(`
name: "bad-types"
steps:
  - id: 123
    strictSkillMatch: "yes"
`);
    await assert.rejects(
      () => ScenarioLoader.load(filePath),
      (err: Error) => {
        assert.ok(err.message.includes('Invalid scenario file'));
        return true;
      }
    );
  });
});
