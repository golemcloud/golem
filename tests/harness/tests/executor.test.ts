import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { ScenarioExecutor } from '../src/executor.js';

type StepLike = { expectedSkills?: string[]; strictSkillMatch?: boolean; allowedExtraSkills?: string[] };
type ExecutorWithPrivate = { assertSkillActivation(step: StepLike, activated: string[]): string | undefined };

function assertSkillActivation(step: StepLike, activatedSkills: string[]): string | undefined {
  const executor = Object.create(ScenarioExecutor.prototype) as unknown as ExecutorWithPrivate;
  return executor.assertSkillActivation(step, activatedSkills);
}

describe('assertSkillActivation', () => {
  it('returns undefined when no expected skills', () => {
    const result = assertSkillActivation({}, ['some-skill']);
    assert.equal(result, undefined);
  });

  it('returns undefined when all expected skills are activated', () => {
    const result = assertSkillActivation(
      { expectedSkills: ['skill-a', 'skill-b'] },
      ['skill-a', 'skill-b']
    );
    assert.equal(result, undefined);
  });

  it('returns error when expected skill is missing', () => {
    const result = assertSkillActivation(
      { expectedSkills: ['skill-a', 'skill-b'] },
      ['skill-a']
    );
    assert.ok(result);
    assert.ok(result.includes('SKILL_NOT_ACTIVATED'));
    assert.ok(result.includes('skill-b'));
  });

  it('allows extra skills by default when allowedExtraSkills not set', () => {
    const result = assertSkillActivation(
      { expectedSkills: ['skill-a'] },
      ['skill-a', 'skill-extra']
    );
    assert.ok(result);
    assert.ok(result.includes('SKILL_MISMATCH'));
  });

  it('allows specified extra skills', () => {
    const result = assertSkillActivation(
      { expectedSkills: ['skill-a'], allowedExtraSkills: ['skill-extra'] },
      ['skill-a', 'skill-extra']
    );
    assert.equal(result, undefined);
  });

  it('returns error for strict match with extras', () => {
    const result = assertSkillActivation(
      { expectedSkills: ['skill-a'], strictSkillMatch: true },
      ['skill-a', 'skill-extra']
    );
    assert.ok(result);
    assert.ok(result.includes('SKILL_MISMATCH'));
    assert.ok(result.includes('strict'));
  });

  it('passes strict match when exact', () => {
    const result = assertSkillActivation(
      { expectedSkills: ['skill-a'], strictSkillMatch: true },
      ['skill-a']
    );
    assert.equal(result, undefined);
  });
});
