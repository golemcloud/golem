import { describe, it, beforeEach } from 'node:test';
import assert from 'node:assert/strict';
import { SkillWatcher } from '../src/watcher.js';

describe('SkillWatcher', () => {
  describe('pathToSkillName', () => {
    it('extracts skill name from a valid SKILL.md path', () => {
      const watcher = new SkillWatcher('/tmp/skills');
      assert.equal(watcher.pathToSkillName('/tmp/skills/adding-dependencies/SKILL.md'), 'adding-dependencies');
    });

    it('extracts skill name from nested path', () => {
      const watcher = new SkillWatcher('/tmp/skills');
      assert.equal(watcher.pathToSkillName('/some/other/path/golem-new-project/SKILL.md'), 'golem-new-project');
    });

    it('returns null for non-SKILL.md files', () => {
      const watcher = new SkillWatcher('/tmp/skills');
      assert.equal(watcher.pathToSkillName('/tmp/skills/adding-dependencies/README.md'), null);
    });

    it('returns null for empty string', () => {
      const watcher = new SkillWatcher('/tmp/skills');
      assert.equal(watcher.pathToSkillName(''), null);
    });
  });

  describe('baseline and since tracking', () => {
    let watcher: SkillWatcher;

    beforeEach(() => {
      watcher = new SkillWatcher('/tmp/skills');
    });

    it('markBaseline returns 0 initially', () => {
      assert.equal(watcher.markBaseline(), 0);
    });

    it('getActivatedEventsSince returns empty for fresh watcher', () => {
      const baseline = watcher.markBaseline();
      assert.deepEqual(watcher.getActivatedEventsSince(baseline), []);
    });

    it('getActivatedSkills returns empty initially', () => {
      assert.deepEqual(watcher.getActivatedSkills(), []);
    });
  });

  describe('clearActivatedSkills', () => {
    it('resets all state', () => {
      const watcher = new SkillWatcher('/tmp/skills');
      watcher.clearActivatedSkills();
      assert.deepEqual(watcher.getActivatedSkills(), []);
      assert.equal(watcher.markBaseline(), 0);
    });
  });
});
