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

  // New tests for #2887 settings/prerequisites

  it('loads scenario with settings', async () => {
    const filePath = await writeTempYaml(`
name: "with-settings"
settings:
  timeout_per_subprompt: 120
  golem_server:
    router_port: 9000
    custom_request_port: 9001
  cleanup: false
steps:
  - prompt: "hello"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.settings?.timeout_per_subprompt, 120);
    assert.equal(spec.settings?.golem_server?.router_port, 9000);
    assert.equal(spec.settings?.golem_server?.custom_request_port, 9001);
    assert.equal(spec.settings?.cleanup, false);
  });

  it('loads scenario with prerequisites', async () => {
    const filePath = await writeTempYaml(`
name: "with-prereqs"
prerequisites:
  env:
    MY_VAR: "test_value"
    OTHER_VAR: "other"
steps:
  - prompt: "hello"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.deepEqual(spec.prerequisites?.env, { MY_VAR: 'test_value', OTHER_VAR: 'other' });
  });

  it('loads step with timeout and continue_session', async () => {
    const filePath = await writeTempYaml(`
name: "step-options"
steps:
  - id: "step-1"
    prompt: "first"
    timeout: 60
  - id: "step-2"
    prompt: "second"
    continue_session: false
    timeout: 120
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.steps[0].timeout, 60);
    assert.equal(spec.steps[1].continue_session, false);
    assert.equal(spec.steps[1].timeout, 120);
  });

  // New tests for #2890 invoke

  it('loads step with invoke and expect', async () => {
    const filePath = await writeTempYaml(`
name: "invoke-test"
steps:
  - id: "invoke-step"
    invoke:
      agent: "my-agent"
      function: "my-func"
      args: '{"key": "value"}'
    expect:
      exit_code: 0
      stdout_contains: "success"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.steps[0].invoke?.agent, 'my-agent');
    assert.equal(spec.steps[0].invoke?.function, 'my-func');
    assert.equal(spec.steps[0].expect?.exit_code, 0);
    assert.equal(spec.steps[0].expect?.stdout_contains, 'success');
  });

  // New tests for #2893 shell/sleep/trigger

  it('loads step with sleep', async () => {
    const filePath = await writeTempYaml(`
name: "sleep-test"
steps:
  - id: "wait"
    sleep: 5
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.steps[0].sleep, 5);
  });

  it('loads step with shell command', async () => {
    const filePath = await writeTempYaml(`
name: "shell-test"
steps:
  - id: "run-cmd"
    shell:
      command: "echo"
      args: ["hello"]
      cwd: "./subdir"
    expect:
      stdout_contains: "hello"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.steps[0].shell?.command, 'echo');
    assert.deepEqual(spec.steps[0].shell?.args, ['hello']);
    assert.equal(spec.steps[0].shell?.cwd, './subdir');
  });

  it('loads step with trigger', async () => {
    const filePath = await writeTempYaml(`
name: "trigger-test"
steps:
  - id: "fire"
    trigger:
      agent: "my-agent"
      function: "do-thing"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.steps[0].trigger?.agent, 'my-agent');
    assert.equal(spec.steps[0].trigger?.function, 'do-thing');
  });

  // New tests for #2894 create/delete agent

  it('loads step with create_agent', async () => {
    const filePath = await writeTempYaml(`
name: "create-agent-test"
steps:
  - id: "create"
    create_agent:
      name: "test-agent"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.steps[0].create_agent?.name, 'test-agent');
  });

  it('loads step with delete_agent', async () => {
    const filePath = await writeTempYaml(`
name: "delete-agent-test"
steps:
  - id: "delete"
    delete_agent:
      name: "test-agent"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.steps[0].delete_agent?.name, 'test-agent');
  });
});