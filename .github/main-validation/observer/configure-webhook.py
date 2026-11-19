"""Operator-only webhook setup/probe. Never called by an observation wake-up."""

import hashlib
import hmac
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import urllib.error
import urllib.request


config_path = Path('.local/main-validation-webhook.json')
repo = 'repos/golemcloud/golem/'


def save(config):
    temporary = config_path.with_suffix('.tmp')
    with os.fdopen(os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600), 'w') as output:
        json.dump(config, output)
    temporary.replace(config_path)


def github(path, payload=None):
    command = ['gh', 'api', repo + path]
    if payload is not None:
        command += ['--method', 'POST', '--input', '-']
    result = subprocess.run(command, input=json.dumps(payload) if payload is not None else None,
                            text=True, capture_output=True, timeout=20)
    if result.returncode:
        # gh diagnostics can contain the credential-bearing request URL/body.
        raise SystemExit('GitHub API operation failed; check repository webhook permissions. No automatic write retry.')
    return json.loads(result.stdout)


def main():
    if sys.argv[1:2] == ['init'] and len(sys.argv) == 4:
        if config_path.exists():
            raise SystemExit('Configuration exists; preserve its secret and registration')
        state = Path(sys.argv[3])
        if not state.is_absolute() or not state.is_dir() or not sys.argv[2].startswith('T-'):
            raise SystemExit('Specify the existing owner thread and its absolute state directory')
        config_path.parent.mkdir(exist_ok=True, mode=0o700)
        save({'ownerThread': sys.argv[2], 'stateDir': str(state), 'secret': secrets.token_hex(32)})
        print('Private webhook configuration initialized; load the plugin in the existing owner thread')
        return
    config = json.loads(config_path.read_text())
    if sys.argv[1:] == ['register']:
        # Inspect first, including after an ambiguous previous create. Do not create two hooks.
        hooks = github('hooks?per_page=100')
        matching = [hook for hook in hooks if hook.get('config', {}).get('url') == config['url']]
        if len(matching) > 1 or len(hooks) == 100:
            raise SystemExit('Ambiguous hook inventory; operator inspection required')
        if matching:
            hook = matching[0]
            if (not hook['active'] or hook['events'] != ['workflow_run']
                    or hook['config'].get('content_type') != 'json'
                    or hook['config'].get('insecure_ssl') != '0'):
                raise SystemExit('Existing hook has different settings; operator inspection required')
        else:
            hook = github('hooks', {'name': 'web', 'active': True, 'events': ['workflow_run'],
                                   'config': {'url': config['url'], 'content_type': 'json',
                                              'insecure_ssl': '0', 'secret': config['secret']}})
        config['hookId'] = hook['id']
        save(config)
        print(f"Repository workflow_run webhook registered: {hook['id']}")
    elif sys.argv[1:2] == ['probe'] and len(sys.argv) == 4:
        run_id, attempt = int(sys.argv[2]), int(sys.argv[3])
        seen = Path(config['stateDir']) / 'seen' / f'{run_id}-a{attempt}.json'
        if not seen.is_file():
            raise SystemExit('Probe requires an already processed real attempt; never remove its seen marker')
        run = github(f'actions/runs/{run_id}/attempts/{attempt}')
        if run['path'] != '.github/workflows/main-validation.yaml' or run['status'] != 'completed':
            raise SystemExit('Probe requires an existing completed Main validation attempt')
        body = json.dumps({'action': 'completed', 'repository': {'full_name': 'golemcloud/golem'},
                           'workflow_run': run}).encode()
        signature = 'sha256=' + hmac.new(config['secret'].encode(), body, hashlib.sha256).hexdigest()
        # Separate delivery IDs exercise semantic run/attempt deduplication, not HTTP deduplication.
        for _ in range(2):
            request = urllib.request.Request(config['url'], data=body, method='POST', headers={
                'Content-Type': 'application/json', 'X-GitHub-Event': 'workflow_run',
                'X-Hub-Signature-256': signature, 'Idempotency-Key': secrets.token_hex(16)})
            try:
                with urllib.request.urlopen(request, timeout=20) as response:
                    print(f'Signed processed-attempt probe queued: HTTP {response.status}')
            except (urllib.error.URLError, TimeoutError):
                raise SystemExit('Webhook probe failed; inspect the private registration without printing its URL')
        print(f'Confirm events/{run_id}-a{attempt}.json has status seen; HTTP 200 alone proves only enqueue')
    else:
        raise SystemExit('usage: configure-webhook.py init <owner-thread> <state-dir> | register | probe <run> <attempt>')


if __name__ == '__main__':
    main()
