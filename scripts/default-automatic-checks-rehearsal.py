#!/usr/bin/env python3
"""Exercise sequential opt-in default-branch Git HTTP Checks in a disposable installation."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace('-', '_'), Path(__file__).with_name(name + '.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


manual = load('http-local-checks-rehearsal')


class Rehearsal(manual.Rehearsal):
    def lifecycle(self, *, expected=200, code=None, client=None, repo_id=None):
        client = client or self.f.admin
        repo_id = repo_id or self.f.ids[0]
        headers = {'Authorization': 'Bearer ' + client.token} if client.token else {}
        request = urllib.request.Request(client.url + f'/api/v1/repos/{repo_id}/local-check-automatic-runs', headers=headers)
        try:
            response = client.opener.open(request, timeout=30)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            body, status, cache = response.read(3 * 1024 * 1024 + 1), response.status, response.headers.get('Cache-Control')
        self.h.require(status == expected and cache == 'no-store' and len(body) <= 3 * 1024 * 1024,
                       'automatic lifecycle status, cache or response bound differs')
        self.h.require(str(self.f.root).encode() not in body, 'automatic lifecycle exposed a host path')
        if expected != 200:
            self.h.require(code is not None and json.loads(body) == {'code': code},
                           'automatic lifecycle rejection differs from its fixed error envelope')
            return None
        value = json.loads(body)
        self.h.require(set(value) == {'version', 'repo_id', 'runs'} and value['version'] == 1
                       and value['repo_id'] == repo_id and isinstance(value['runs'], list)
                       and len(value['runs']) <= 7, 'unexpected automatic lifecycle envelope')
        keys = {'run_id', 'source_ref', 'expected_commit', 'checks_blob_oid', 'phase',
                'reason', 'history', 'created_at', 'updated_at'}
        self.h.require(all(set(run) == keys for run in value['runs'])
                       and len({run['run_id'] for run in value['runs']}) == len(value['runs']),
                       'automatic lifecycle row schema or run identity differs')
        return value

    def cleanup_state(self):
        state = self.cli.state(self.cli.repo)
        registration = []
        for block in state[2].split('\n\n'):
            lines = block.splitlines()
            if 'worktree ' + str(self.cli.repo) in lines:
                lines = [line for line in lines if not line.startswith('HEAD ')]
            registration.append(tuple(lines))
        agent_refs = tuple(line for line in state[1].splitlines() if line.startswith('refs/heads/agent/'))
        return (state[0], tuple(registration), agent_refs, state[3], *state[4:])

    def refs(self):
        return dict(line.split('\0', 1) for line in self.cli.git(
            self.cli.repo, 'for-each-ref', '--format=%(refname)%00%(objectname)').splitlines())

    def unchanged_refs(self):
        self.h.require(self.refs() == self.expected_refs,
                       'Checks changed refs beyond the explicitly accepted Git client updates')

    def auth_env(self):
        return {'GIT_CONFIG_COUNT': '1', 'GIT_CONFIG_KEY_0': 'http.extraHeader',
                'GIT_CONFIG_VALUE_0': 'Authorization: Bearer ' + self.f.admin.token,
                'NO_PROXY': '127.0.0.1', 'no_proxy': '127.0.0.1'}

    def push(self, *, commands, enabled, label):
        expected_refs = self.refs()
        (self.client / '.gitust-checks.json').write_text(json.dumps(
            {'version': 1, 'commands': commands, 'auto_run': enabled}) + '\n')
        (self.client / 'change.txt').write_text(label + '\n')
        self.cli.git(self.client, 'add', '.gitust-checks.json', 'change.txt')
        self.cli.git(self.client, 'commit', '-m', 'test: ' + label)
        expected = self.cli.git(self.client, 'rev-parse', 'HEAD')
        expected_refs['refs/heads/main'] = expected
        self.expected_refs = expected_refs
        self.h.git(self.client, 'push', 'origin', 'HEAD:refs/heads/main', env=self.auth_env())
        self.h.require(self.cli.git(self.f.repos[0], 'rev-parse', 'refs/heads/main') == expected,
                       'accepted ordinary Git push differs from the actual primary ref')
        self.h.git(self.client, 'fetch', 'origin', env=self.auth_env())
        self.h.require(self.cli.git(self.client, 'rev-parse', 'origin/main') == expected,
                       'ordinary Git fetch differs from the accepted primary commit')
        return expected

    def completed(self, expected, commands, codes):
        # Wait only for an ordinary accepted command to finish; no concurrent mutation is introduced.
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            history = self.read(self.f.admin, self.f.ids[0])
            rows = [row for row in history['records'] if row['checked_commit'] == expected]
            if rows:
                row = rows[0]
                runs = self.lifecycle()['runs']
                matching = [run for run in runs if run['run_id'] == row['result']['run_id']]
                if matching and matching[0]['phase'] in ('completed', 'failed'):
                    break
            time.sleep(0.1)
        else:
            raise RuntimeError('accepted automatic Checks did not reach a saved terminal result')
        result = row['result']
        phase = 'completed' if codes[0] == 0 else 'failed'
        reason = None if codes[0] == 0 else 'command_failed'
        lifecycle = matching[0]
        self.h.require(lifecycle['source_ref'] == row['source_ref']
                       and lifecycle['expected_commit'] == expected
                       and lifecycle['checks_blob_oid'] == row['checks_blob_oid']
                       and lifecycle['phase'] == phase and lifecycle['reason'] == reason
                       and lifecycle['history'] == 'saved',
                       'automatic lifecycle does not exactly identify the saved command outcome')
        self.h.require(len(rows) == 1 and len(matching) == 1 and row['source_ref'] == 'refs/heads/main'
                       and row['checks_blob_oid'] == self.cli.git(self.client, 'rev-parse', expected + ':.gitust-checks.json')
                       and result['exit_code'] == codes[0] and result['network_isolated'] is True
                       and [step['command'] for step in result['steps']] == commands
                       and [step['exit_code'] for step in result['steps']] == codes
                       and all(step['required'] for step in result['steps']),
                       'automatic result differs from the accepted committed policy or actual command results')
        self.h.require(result['run_id'] not in self.run_ids, 'automatic attempts reused an existing result ID')
        self.run_ids.append(result['run_id'])
        self.cli.backends.add(result['backend'])
        self.unchanged_refs()
        self.h.require(self.cleanup_state() == self.original_cleanup,
                       'automatic Checks changed native worktree registrations, checkout/scratch entries or approval state')
        self.h.require(not list((self.cli.common(self.cli.repo) / 'gitust-active-executions').glob('*.json')),
                       'completed automatic execution left an active execution fence')
        return row

    def backup_restore(self, history, lifecycle):
        self.f.stop(self.f.process)
        refs = [self.cli.git(repo, 'for-each-ref', '--format=%(refname)%00%(objectname)') for repo in self.f.repos]
        authority = self.cli.authority(self.cli.repo)
        snapshot, restored = self.f.root / 'backup', self.f.root / 'restored'
        tool = Path(__file__).with_name('backup.py')
        self.h.command([sys.executable, str(tool), 'create', '--repo', str(self.f.repos[0]),
                        '--read-only-repo', str(self.f.repos[1]), '--auth-db', str(self.f.auth), '--output', str(snapshot)])
        self.h.command([sys.executable, str(tool), 'restore', '--backup', str(snapshot), '--destination', str(restored)])
        original_repos = list(self.f.repos)
        original_auth = self.f.auth
        for path in original_repos:
            path.rename(path.with_name(path.name + '-offline'))
        original_auth.parent.rename(original_auth.parent.with_name('auth-offline'))
        self.f.repos = [restored / 'repository', restored / 'read-only/0001/repository']
        self.f.auth = restored / 'auth/auth-db.json'
        self.cli.repo = self.f.repos[0]
        self.restart(trusted=False)
        self.h.require(self.read(self.f.admin, self.f.ids[0]) == history and self.lifecycle() == lifecycle,
                       'offline relocated restore changed completed results or automatic lifecycle')
        self.h.require([self.cli.git(repo, 'for-each-ref', '--format=%(refname)%00%(objectname)') for repo in self.f.repos] == refs
                       and self.cli.authority(self.cli.repo) == authority,
                       'offline relocated restore changed original refs, tasks or approval records')
        for repo in self.f.repos:
            self.cli.git(repo, 'fsck', '--full', '--strict')

    def execute(self):
        self.run_ids = []
        self.cli.git(self.f.root, 'init', '--bare', str(self.f.root / 'unused-upstream.git'))
        self.f.setup()
        self.f.stop(self.f.process)
        self.seed_existing_tasks()
        self.restart(trusted=True)
        self.original_cleanup = self.cleanup_state()
        self.client = self.f.root / 'git-client'
        self.h.git(self.f.root, 'clone', self.f.url + '/git/' + self.f.ids[0] + '.git', str(self.client), env=self.auth_env())
        self.cli.git(self.client, 'config', 'user.name', 'Gitust Automatic Checks')
        self.cli.git(self.client, 'config', 'user.email', 'automatic@company.internal')
        self.h.require(self.lifecycle()['runs'] == [] and self.read(self.f.admin, self.f.ids[0])['records'] == [],
                       'fresh installation already has automatic attempts or completed results')
        initial = ['printf original-policy']
        self.push(commands=initial, enabled=True, label='activate default branch automatic Checks')
        self.h.require(self.lifecycle()['runs'] == [] and self.read(self.f.admin, self.f.ids[0])['records'] == [],
                       'first policy activation executed without prior committed consent')
        self.unchanged_refs()
        passed = ['test "$(cat change.txt)" = "accepted changed policy" && printf accepted-snapshot', 'printf second-check']
        expected = self.push(commands=passed, enabled=True, label='accepted changed policy')
        self.completed(expected, passed, [0, 0])
        failed = ['printf expected-failure; exit 7', 'printf skipped-command']
        expected = self.push(commands=failed, enabled=True, label='accepted failed Check')
        failure = self.completed(expected, failed, [7, None])
        self.h.require('skipped-command' not in failure['result']['stdout']
                       and failure['result']['steps'][1].get('output') is None,
                       'failed required Check ran a later skipped command')
        saved = self.read(self.f.admin, self.f.ids[0])
        disabled = self.push(commands=passed, enabled=False, label='disable automatic Checks')
        self.h.require(self.read(self.f.admin, self.f.ids[0]) == saved,
                       'accepted policy opt-out executed a command')
        stopped = self.lifecycle()
        opt_out = [run for run in stopped['runs'] if run['expected_commit'] == disabled]
        self.h.require(len(opt_out) == 1 and opt_out[0]['source_ref'] == 'refs/heads/main'
                       and opt_out[0]['phase'] == 'rejected' and opt_out[0]['reason'] == 'policy_disabled'
                       and opt_out[0]['history'] == 'not_attempted',
                       'accepted opt-out did not retain a precise non-executed terminal attempt')
        self.unchanged_refs()
        self.push(commands=passed, enabled=True, label='reenable for subsequent pushes')
        self.h.require(self.lifecycle() == stopped and self.read(self.f.admin, self.f.ids[0]) == saved,
                       'reenabling without prior consent scheduled an attempt')
        self.unchanged_refs()
        for ref in ('refs/heads/other', 'refs/tags/ordinary'):
            self.expected_refs[ref] = self.cli.git(self.client, 'rev-parse', 'HEAD')
        self.h.git(self.client, 'push', 'origin', 'HEAD:refs/heads/other', 'HEAD:refs/tags/ordinary', env=self.auth_env())
        self.h.git(self.client, 'push', 'origin', 'HEAD:refs/heads/main', env=self.auth_env())
        self.h.require(self.lifecycle() == stopped and self.read(self.f.admin, self.f.ids[0]) == saved,
                       'non-default branch, tag or no-op push scheduled default Checks')
        self.unchanged_refs()
        self.lifecycle(expected=403, code='local_checks_automatic_forbidden', client=self.f.members[0][0])
        self.lifecycle(expected=404, code='local_checks_automatic_not_hosted', repo_id=self.f.ids[1])
        self.restart(trusted=False)
        self.h.require(self.lifecycle() == stopped and self.read(self.f.admin, self.f.ids[0]) == saved,
                       'restart with execution disabled changed durable diagnostics')
        self.cli.git(self.client, 'fsck', '--full', '--strict')
        self.backup_restore(saved, stopped)
        return {'version': 1, 'passed': True, 'actual_repositories': 2,
                'real_clone_push_fetch': True, 'prior_committed_opt_in_required': True,
                'accepted_policy_snapshot': True, 'success_failure_and_skipped_steps': True,
                'stable_attempt_result_identity': True, 'opt_out_stops_execution': True,
                'non_default_tag_and_noop_ignored': True, 'primary_siteadmin_diagnostics': True,
                'bounded_no_store_responses': True, 'disabled_restart_preserves_diagnostics': True,
                'offline_relocated_restore_preserves_diagnostics': True, 'git_fsck': True,
                'confirmed_cleanup': True, 'platform': os.uname().sysname,
                'sandbox_backends': sorted(self.cli.backends), 'external_github_publication': False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=Path('target/debug/gitust'))
    parser.add_argument('--source', type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    h = load('alpha-rehearsal')
    source, binary = args.source.resolve(strict=True), args.binary.resolve(strict=True)
    h.require(not args.report.exists() and not args.report.is_symlink(), 'report already exists')
    commit, dirty = h.git(source, 'rev-parse', 'HEAD'), bool(h.git(source, 'status', '--porcelain'))
    names = [Path(__file__).name, 'alpha-rehearsal.py', 'multi-repository-rehearsal.py',
             'local-checks-web-rehearsal.py', 'http-local-checks-rehearsal.py',
             'cli-checks-rehearsal.py', 'cli-checks-history-rehearsal.py', 'backup.py']
    paths = [Path(__file__).with_name(name).resolve() for name in names]
    runtime = {'scripts/' + path.name: h.sha256(path) for path in paths}
    for name, digest in runtime.items():
        old = subprocess.run(['git', '-C', str(source), 'show', commit + ':' + name],
                             env=h.clean_environment(), capture_output=True, timeout=30)
        dirty |= old.returncode != 0 or hashlib.sha256(old.stdout).hexdigest() != digest
    binary_sha = h.sha256(binary)
    with tempfile.TemporaryDirectory(prefix='gitust-default-automatic-') as directory:
        scratch = Path(directory).resolve()
        fixture = manual.Fixture(h, binary, scratch)
        try:
            report = Rehearsal(h, fixture).execute()
        finally:
            for process in reversed(fixture.processes):
                fixture.stop(process)
            h.require(all(process.poll() is not None for process in fixture.processes), 'owned services were not reaped')
    h.require(not scratch.exists(), 'private fixture was not removed')
    h.require(runtime == {'scripts/' + path.name: h.sha256(path) for path in paths}
              and h.sha256(binary) == binary_sha and h.git(source, 'rev-parse', 'HEAD') == commit,
              'runtime source, binary or commit changed during rehearsal')
    report.update({'source_commit': commit, 'runtime_worktree_dirty': dirty or bool(h.git(source, 'status', '--porcelain')),
                   'binary_sha256': binary_sha, 'runtime_source_sha256': runtime,
                   'private_fixture_removed': True, 'owned_services_reaped': True})
    with os.fdopen(os.open(args.report, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), 'w') as output:
        json.dump(report, output, indent=2)
        output.write('\n')
    print('Default-branch automatic Checks passed: real Git transport, saved results and relocated diagnostics.')


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print('Default automatic Checks failed:', str(error) if isinstance(error, RuntimeError) else type(error).__name__)
        raise SystemExit(1) from None
