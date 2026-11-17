#!/usr/bin/env python3
"""Opt-in interactive macOS contract test using our own NSTextInputClient app, never a terminal."""
import json
import os
from pathlib import Path
import plistlib
import socket
import shutil
import subprocess
import sys
import time
import uuid

ROOT = Path(__file__).resolve().parent.parent
if len(sys.argv) < 2 or '--manual-enrollment' not in sys.argv:
    raise SystemExit('Usage: test-input-method-session.py OUTPUT --manual-enrollment [--manual-focus]\n'
                     'A new fixture input source needs explicit approval in Keyboard Settings; '
                     'headless --enable is not supported.')
OUT = Path(sys.argv[1]).resolve()
OUT.mkdir(parents=True, exist_ok=True)
TEST_TOKEN = uuid.uuid4().hex
HELPER = OUT / f'FigInputMethodTest-{TEST_TOKEN}.app'
CLIENT = OUT / 'FigIMEFixture.app'
TEST_ID = f'local.openfig.inputmethod.figcontract.{TEST_TOKEN}'
CONNECTION_NAME = f'{TEST_ID}.connection'
LINK = Path.home() / 'Library/Input Methods' / HELPER.name
SOCKET = Path(subprocess.check_output(['getconf', 'DARWIN_USER_TEMP_DIR'], text=True).strip()) / 'figrun/ime-test.sock'
STATE = OUT / 'state.json'
COMMAND = OUT / 'command.json'


def run(*args):
    try:
        return subprocess.check_output(args, text=True, stderr=subprocess.STDOUT).strip()
    except subprocess.CalledProcessError as error:
        print(error.output, file=sys.stderr)
        raise


def wait_for(read, predicate, seconds=10, interval=.05):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        try:
            value = read()
            if predicate(value):
                return value
        except (FileNotFoundError, json.JSONDecodeError):
            pass
        time.sleep(interval)
    raise RuntimeError('Timed out waiting for fixture state')


if LINK.exists() or LINK.is_symlink() or SOCKET.exists():
    raise SystemExit('A fixture source/socket already exists; inspect it before rerunning')
for bundle in [HELPER, CLIENT]:
    (bundle / 'Contents/MacOS').mkdir(parents=True, exist_ok=True)
(HELPER / 'Contents/Resources').mkdir(exist_ok=True)
shutil.copyfile(ROOT / 'bundle/macos/AppIcon.icns', HELPER / 'Contents/Resources/AppIcon.icns')
helper = HELPER / 'Contents/MacOS/fig_input_method_test'
client = CLIENT / 'Contents/MacOS/fig_ime_fixture'
flags = ['xcrun', 'clang', '-fobjc-arc', '-Wall', '-Wextra', '-Werror', '-O2']
run(*flags, '-DFIG_IME_FIXTURE=1', f'-DFIG_IME_TEST_BUNDLE_ID=@"{TEST_ID}"',
    f'-DFIG_IME_TEST_CONNECTION_NAME=@"{CONNECTION_NAME}"',
    str(ROOT / 'native/input-method/main.m'), '-framework', 'Cocoa', '-framework', 'Carbon', '-framework', 'InputMethodKit', '-o', str(helper))
run(*flags, str(ROOT / 'native/input-method/tests/client.m'), '-framework', 'Cocoa', '-o', str(client))
info = plistlib.loads((ROOT / 'native/input-method/Info.plist').read_bytes())
for key in ['CFBundleIdentifier', 'TISInputSourceID']:
    info[key] = TEST_ID
info['InputMethodConnectionName'] = CONNECTION_NAME
info['CFBundleExecutable'] = 'fig_input_method_test'
info['ComponentInvisibleInSystemUI'] = False
(HELPER / 'Contents/Info.plist').write_bytes(plistlib.dumps(info))
(CLIENT / 'Contents/Info.plist').write_bytes(plistlib.dumps({
    'CFBundleIdentifier': 'local.openfig.imefixture', 'CFBundleName': 'OpenFig IMK Fixture',
    'CFBundleExecutable': 'fig_ime_fixture', 'CFBundlePackageType': 'APPL', 'CFBundleVersion': '1'}))
identities = run('security', 'find-identity', '-v', '-p', 'codesigning')
import re
identity = next((name for name in re.findall(r'"([^"]+)"', identities) if name.startswith(('Apple Development:', 'Developer ID Application:'))), '-')
for bundle in [HELPER, CLIENT]:
    run('codesign', '--force', '--sign', identity, str(bundle))
report = {'source_id': TEST_ID, 'installed_bundle': str(LINK), 'phase': 'registration',
          'keyboard_before': run(str(helper), '--keyboard-source'), 'cases': []}
process = None
helper_process = None
server = socket.socket(socket.AF_UNIX)
connection = None
try:
    LINK.parent.mkdir(parents=True, exist_ok=True)
    shutil.copytree(HELPER, LINK)
    helper = LINK / "Contents/MacOS/fig_input_method_test"
    SOCKET.parent.mkdir(exist_ok=True)
    server.bind(str(SOCKET))
    os.chmod(SOCKET, 0o600)
    server.listen(1)
    server.settimeout(15)
    report['register'] = run(str(helper), '--register', str(LINK))
    # Source ID alone may resolve stale registration data after replacing a bundle.
    report['registered_status'] = json.loads(run(str(helper), '--status'))
    report['phase'] = 'enrollment'
    print(f'Enable the fixture source in Keyboard Settings: {TEST_ID}', flush=True)
    wait_for(lambda: json.loads(run(str(helper), '--status')), lambda s: s['enabled'], seconds=600, interval=1)
    report['select'] = run(str(helper), '--select')
    report['status'] = json.loads(run(str(helper), '--status'))
    assert report['status']['enabled'] and report['status']['selected'], report['status']
    report['phase'] = 'client-session'
    with (OUT / 'helper.log').open('w') as log:
        helper_process = subprocess.Popen([str(helper)], stdout=log, stderr=log)
    connection, _ = server.accept()
    connection.settimeout(5)
    reader = connection.makefile('rb')
    COMMAND.write_text('{"mode":0}')
    with (OUT / 'client.log').open('w') as log:
        process = subprocess.Popen([str(client), str(STATE), str(COMMAND)], stdout=log, stderr=log)
    state = wait_for(lambda: json.loads(STATE.read_text()), lambda s: s['window'] > 0)
    if '--manual-focus' in sys.argv:
        print(f'Waiting for focus on {CLIENT}', flush=True)
        state = wait_for(lambda: json.loads(STATE.read_text()), lambda s: s['front'] == s['pid'], seconds=60)
    for mode in [0, 1, 2]:
        COMMAND.write_text(json.dumps({'mode': mode}))
        state = wait_for(lambda: json.loads(STATE.read_text()), lambda s: s['mode'] == mode)
        before = state['calls']
        reply = None
        # Session activation is asynchronous. Null during startup is not a successful coordinate.
        for attempt in range(20):
            identifier = str(uuid.uuid4())
            query = {'v': 1, 'type': 'query', 'id': identifier, 'pid': state['pid'], 'window': state['window'], 'bundle': 'local.openfig.imefixture'}
            connection.sendall((json.dumps(query) + '\n').encode())
            while True:
                message = json.loads(reader.readline())
                if message['type'] == 'rect':
                    assert message['id'] == identifier
                    reply = message
                    break
            if reply['rect'] is not None:
                break
            time.sleep(.1)
        assert reply['rect'] is not None, {'state': state, 'reply': reply}
        assert all(abs(a - b) < .05 for a, b in zip(reply['rect'], state['expected'])), (state, reply)
        observed = wait_for(lambda: json.loads(STATE.read_text()), lambda s: s['calls'] > before)
        assert observed['index'] == 0, observed
        report['cases'].append({'mode': mode, 'expected': state['expected'], 'actual': reply['rect'], 'firstRect_calls': observed['calls'] - before})
    report['keyboard_after'] = run(str(helper), '--keyboard-source')
    assert report['keyboard_before'] == report['keyboard_after'], report
    report['result'] = 'passed'
except BaseException as error:
    report['result'] = 'failed'
    report['error'] = repr(error)
    raise
finally:
    cleanup_errors = []
    try:
        COMMAND.write_text('{"quit":true}')
        if process:
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.terminate()
                process.wait(timeout=5)
    except Exception as error:
        cleanup_errors.append(f'client: {error!r}')
    try:
        if LINK.exists():
            report['disable'] = run(str(helper), '--disable')
            if helper_process and helper_process.poll() is None:
                helper_process.terminate()
                helper_process.wait(timeout=5)
            report['keyboard_after_cleanup'] = run(str(helper), '--keyboard-source')
            if report['keyboard_after_cleanup'] != report['keyboard_before']:
                cleanup_errors.append('Keyboard source changed during the fixture test')
            shutil.rmtree(LINK)
    except Exception as error:
        # Leave the owned bundle available for manual cleanup if deregistration fails.
        cleanup_errors.append(f'input source: {error!r}')
    if connection:
        connection.close()
    server.close()
    SOCKET.unlink(missing_ok=True)
    report['cleanup_errors'] = cleanup_errors
    (OUT / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
    if cleanup_errors:
        raise RuntimeError(cleanup_errors)
print(json.dumps(report, indent=2))
