#!/usr/bin/env python3
"""Test first-run client installation and publishing without herdr or discovery."""
import json
import os
from pathlib import Path
import selectors
import shlex
import subprocess
import sys
import tempfile
import tomllib

binary = str(Path(sys.argv[1]).resolve())
env = {k: v for k, v in os.environ.items() if not k.startswith(('HERDR_', 'FEEDR_'))}
children = []
with tempfile.TemporaryDirectory(prefix='fdr-agent-', dir='/tmp') as temporary:
    root = Path(temporary)
    data = root / 'data'
    project = root / 'project with spaces'
    project.mkdir()
    base = [binary, '--data-dir', str(data)]

    def cli(*args):
        result = subprocess.run(base + list(args), capture_output=True, text=True, env=env, timeout=15)
        assert result.returncode == 0, result.stderr
        return json.loads(result.stdout)

    try:
        cli('start')
        assert not cli('discovery', 'status')['enabled']
        for client, label, config_file in [('codex', 'Codex', 'config.toml'), ('claude', 'Claude Code', '.claude.json')]:
            profile = root / client
            status = cli('agent-setup', 'enable', '--client', client, '--config-dir', str(profile))
            assert status['connection'] and status['instructions'] and status['reminders'], status
            if client == 'codex':
                config = tomllib.loads((profile / config_file).read_text())['mcp_servers']['feedr']
                hooks = json.loads((profile / 'hooks.json').read_text())
            else:
                config = json.loads((profile / config_file).read_text())['mcpServers']['feedr']
                hooks = json.loads((profile / 'settings.json').read_text())
            payload = {'hook_event_name': 'SessionStart', 'session_id': 'fixture-' + client, 'cwd': str(project)}

            def hook(event, input_payload, data_override=None):
                command = shlex.split(hooks['hooks'][event][-1]['hooks'][0]['command'])
                if data_override is not None:
                    command[command.index('--data-dir') + 1] = str(data_override)
                result = subprocess.run(command, input=json.dumps(input_payload), text=True, capture_output=True, env=env, timeout=5)
                assert result.returncode == 0, result.stderr
                return json.loads(result.stdout)

            startup = hook('SessionStart', payload)
            instruction = startup['hookSpecificOutput']['additionalContext']
            context = json.loads(instruction.split('Your publishing context (supply this as context to feedr_post): ', 1)[1])
            assert context['agent'] == label and context['native_session_id'] == payload['session_id']
            assert context['herdr_socket'] is None and context['pane_id'] is None
            assert 'herdr and mention discovery are optional' in instruction
            report = project / ('report-' + client + '.md')
            report.write_text('# A finished report\n\nReady to review.\n')
            stop = payload | {'hook_event_name': 'Stop', 'turn_id': 'turn-one', 'stop_hook_active': False,
                              'last_assistant_message': f'Report ready: [open](<{report}>)'}
            reminder = hook('Stop', stop)
            assert reminder['decision'] == 'block' and str(report) in reminder['reason']
            assert hook('Stop', stop) == {}, 'Same missed share must not cause a loop'
            assert hook('Stop', stop | {'stop_hook_active': True}) == {}
            assert hook('Stop', stop | {'turn_id': 'offline'}, root / 'offline') == {}
            assert hook('Stop', stop | {'last_assistant_message': 'The explanation is complete.'}) == {}
            assert hook('Stop', stop | {'last_assistant_message': f'Report ready: [missing](<{project / "missing.md"}>)'}) == {}
            source = project / 'source.rs'
            source.write_text('fn main() {}\n')
            assert hook('Stop', stop | {'last_assistant_message': f'Created [source](<{source}>)'}) == {}
            preview = hook('Stop', stop | {'turn_id': 'web-preview', 'last_assistant_message': 'Preview ready: https://example.com/my-app'})
            assert preview['decision'] == 'block'

            mcp_env = env | config['env'] | {'CODEX_THREAD_ID': payload['session_id']}
            mcp = subprocess.Popen([config['command']] + config['args'], cwd=project, env=mcp_env,
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   text=True, bufsize=1)
            children.append(mcp)
            selector = selectors.DefaultSelector()
            selector.register(mcp.stdout, selectors.EVENT_READ)

            def rpc(message):
                mcp.stdin.write(json.dumps(message) + '\n')
                mcp.stdin.flush()
                if 'id' not in message:
                    return None
                assert selector.select(10), 'MCP response timed out'
                response = json.loads(mcp.stdout.readline())
                assert response['id'] == message['id'] and 'error' not in response, response
                return response['result']

            hello = rpc({'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {
                'protocolVersion': '2025-06-18', 'capabilities': {}, 'clientInfo': {'name': 'agent-setup-smoke', 'version': '1'}}})
            assert 'herdr and mention discovery are optional' in hello['instructions']
            rpc({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
            result = rpc({'jsonrpc': '2.0', 'id': 2, 'method': 'tools/call', 'params': {
                'name': 'feedr_post', 'arguments': {'title': client + ' report', 'context': context,
                'resources': [{'uri': str(report)}], 'idempotency_key': 'finished-report'}}})
            assert not result.get('isError'), result
            post = json.loads(result['content'][0]['text'])
            assert post['context']['agent'] == label and post['context']['native_session_id'] == payload['session_id']
            assert Path(cli('get', post['id'])['resources'][0]['target']).resolve() == report.resolve()
            assert hook('Stop', stop | {'turn_id': 'after-post'}) == {}
            assert hook('Stop', stop | {'session_id': 'another-session', 'turn_id': 'another'})['decision'] == 'block', 'Another session must not satisfy this session\'s receipt'
            # An older post does not hide a revised artifact from the completion check.
            import time
            os.utime(report, (time.time() + 3, time.time() + 3))
            assert hook('Stop', stop | {'turn_id': 'revised'})['decision'] == 'block'
            os.utime(report, None)
            result = rpc({'jsonrpc': '2.0', 'id': 3, 'method': 'tools/call', 'params': {
                'name': 'feedr_update', 'arguments': {'id': post['id'], 'expected_revision': post['revision'], 'body': 'Revised report'}}})
            assert not result.get('isError'), result
            assert hook('Stop', stop | {'turn_id': 'after-update'}) == {}
            mcp.terminate()
            mcp.wait(timeout=5)
            assert not cli('discovery', 'status')['enabled']
            print(f'PASS: {label} setup, native session context, standalone MCP publishing and revision-aware bounded reminders', flush=True)
        one = cli('agent-setup', 'test')
        two = cli('agent-setup', 'test')
        assert one['id'] == two['id']
        assert one['context']['label'] == 'Feedr setup'
        assert not cli('discovery', 'status')['enabled']
        print('PASS: first-run test share works without herdr/discovery and is deduplicated', flush=True)
    finally:
        for child in children:
            if child.poll() is None:
                child.terminate()
                child.wait(timeout=5)
        subprocess.run(base + ['stop'], env=env, capture_output=True, timeout=5)
