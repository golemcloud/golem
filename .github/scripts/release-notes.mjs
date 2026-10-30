import { execFileSync } from 'node:child_process';

const repository = 'golemcloud/golem';
const [tag, previousTag] = process.argv.slice(2);
if (!tag) throw new Error('Usage: release-notes.mjs <tag> [previous-tag]');

function preparationPull(releaseTag) {
  const match = /^v(\d+)\.(\d+)\.\d+(?:-[0-9A-Za-z.-]+)?$/.exec(releaseTag);
  if (!match) throw new Error(`Invalid Golem release tag: ${releaseTag}`);
  const head = execFileSync('git', ['rev-parse', `refs/tags/${releaseTag}^{commit}`], {
    encoding: 'utf8',
  }).trim();
  const pages = JSON.parse(execFileSync('gh', [
    'api', '--paginate', '--slurp', `repos/${repository}/commits/${head}/pulls?per_page=100`,
  ], {
    encoding: 'utf8',
    // The publication App has contents access; the workflow token can read PRs.
    env: { ...process.env, GH_TOKEN: process.env.GITHUB_TOKEN || process.env.GH_TOKEN },
  }));
  const pulls = pages.flat().filter(pull => pull.merged_at && pull.head.sha === head &&
    pull.base.repo.full_name === repository &&
    ['main', `${match[1]}.${match[2]}.x`].includes(pull.base.ref));
  if (pulls.length !== 1) {
    throw new Error(`Expected one merged preparation PR for ${releaseTag}, found ${pulls.length}`);
  }
  return pulls[0];
}

const current = preparationPull(tag);
const previous = previousTag ? preparationPull(previousTag) : undefined;
const args = ['api', `repos/${repository}/releases/generate-notes`, '-f', `tag_name=${tag}`];
if (previousTag) args.push('-f', `previous_tag_name=${previousTag}`);
const { body } = JSON.parse(execFileSync('gh', args, { encoding: 'utf8' }));

function mentionsPull(line, pull) {
  return /^\s*[-*]\s/.test(line) &&
    new RegExp(`https://github\\.com/${repository}/pull/${pull.number}(?![0-9])`).test(line);
}

const generated = body.split('\n');
const heading = generated.indexOf("## What's Changed");
if (heading === -1) throw new Error('Generated notes have no What\'s Changed section');
const nextSection = generated.findIndex((line, index) => index > heading &&
  (/^## /.test(line) || line.startsWith('**Full Changelog**')));
let included = false;
const lines = generated.filter((line, index) => {
  if (index <= heading || (nextSection !== -1 && index >= nextSection)) return true;
  if (previous && previous.number !== current.number && mentionsPull(line, previous)) return false;
  if (!mentionsPull(line, current)) return true;
  if (included) return false;
  included = true;
  return true;
});
if (!included) {
  const author = current.user?.login ? ` by @${current.user.login}` : '';
  lines.splice(heading + 1, 0,
    `* ${current.title}${author} in https://github.com/${repository}/pull/${current.number}`);
}
process.stdout.write(lines.join('\n'));
