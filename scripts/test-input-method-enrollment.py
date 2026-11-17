#!/usr/bin/env python3
"""Exercise installer transitions without registering sources or launching apps."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

INSTALLER = Path(__file__).with_name('install-input-method.sh').resolve()
HELPER = r'''
import json, os, sys
from pathlib import Path
root = Path(os.environ['FIG_ENROLLMENT_TEST'])
state = json.loads((root / 'state.json').read_text())
command = '--enable' if sys.argv[1:] == ['debug', 'enable-input-method'] else sys.argv[1]
with (root / 'events').open('a') as log:
    log.write(f'{command} pid={os.getpid()}\n')
result = 0
if command == '--check-active':
    result = 0 if state['selected'] and state['enabled'] else 3
elif command == '--check-enabled':
    if state.get('requested') and state['scenario'] == 'delayed':
        state['checks'] += 1
        state['enabled'] = state['checks'] >= 2
    result = 0 if state['enabled'] else 3
elif command == '--enable':
    state['requested'] = True
    if state['scenario'] == 'rejected': result = 1
elif command == '--select':
    if not state['enabled']: raise RuntimeError('Selected before enabled')
    state['selected'] = state['scenario'] != 'selection-lost'
elif command == '--status':
    print(json.dumps(state))
else:
    raise RuntimeError(command)
(root / 'state.json').write_text(json.dumps(state))
sys.exit(result)
'''


class EnrollmentTests(unittest.TestCase):
    def test_registration_requires_an_installed_location(self):
        for path, expected in [
            ('/Applications/Fig.app/Contents/Helpers/FigInputMethod.app', 0),
            (str(Path.home() / 'Library/Input Methods/FigInputMethod.app'), 0),
            ('/Library/Input Methods/FigInputMethod.app', 0),
            ('/Users/example/project/build/FigInputMethod.app', 1),
            ('/Applications-fake/FigInputMethod.app', 1),
        ]:
            with self.subTest(path=path):
                result = subprocess.run(
                    ['bash', '-c', 'source "$1"; is_installed_input_method_path "$2"',
                     'test', str(INSTALLER), path], capture_output=True, text=True)
                self.assertEqual(result.returncode, expected, result.stderr)

    def run_case(self, scenario, enabled=False, selected=False, timeout=0):
        with tempfile.TemporaryDirectory(prefix='fig enrollment ') as directory:
            root = Path(directory)
            state = dict(scenario=scenario, enabled=enabled, selected=selected, checks=0)
            (root / 'state.json').write_text(json.dumps(state))
            helper = root / 'helper'
            helper.write_text(f'#!{sys.executable}\n' + HELPER)
            helper.chmod(0o700)
            environment = dict(os.environ, FIG_ENROLLMENT_TEST=directory)
            script = '''source "$1"
open() {
  printf 'open %s %s\\n' "$1" "$2" >> "$FIG_ENROLLMENT_TEST/events"
  [[ "$FIG_ENROLLMENT_CASE" != 'launch-failed' ]]
}
activate_input_method "$2" "$3" "$4" "$2"
'''
            environment['FIG_ENROLLMENT_CASE'] = scenario
            result = subprocess.run(
                ['bash', '-c', script, 'test', str(INSTALLER), str(helper),
                 str(root / 'Test Input.app'), str(timeout)],
                env=environment, text=True, capture_output=True, timeout=10)
            events = (root / 'events').read_text().splitlines()
            return result, events

    def test_delayed_approval_uses_fresh_processes_and_one_request(self):
        result, events = self.run_case('delayed', timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(events[0].startswith('open -g '))
        self.assertEqual(sum(e.startswith('--enable ') for e in events), 1)
        checks = [e for e in events if e.startswith('--check-enabled ')]
        self.assertGreaterEqual(len(checks), 3)
        self.assertEqual(len(checks), len(set(checks)))
        self.assertTrue(events[-1].startswith('--check-active '))

    def test_noerr_without_activation_times_out_without_selecting(self):
        result, events = self.run_case('pending')
        self.assertEqual(result.returncode, 3)
        self.assertFalse(any(e.startswith('--select ') for e in events))
        self.assertIn('not confirmed', result.stderr)

    def test_already_active_does_not_request_or_reselect(self):
        result, events = self.run_case('active', enabled=True, selected=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(events), 2)

    def test_enabled_only_selects(self):
        result, events = self.run_case('enabled', enabled=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(any(e.startswith('--enable ') for e in events))

    def test_failed_launch_does_not_request_activation(self):
        result, events = self.run_case('launch-failed')
        self.assertEqual(result.returncode, 1)
        self.assertEqual(len(events), 1)

    def test_enable_error_is_preserved(self):
        result, events = self.run_case('rejected')
        self.assertEqual(result.returncode, 1)
        self.assertTrue(events[-1].startswith('--enable '))

    def test_selection_must_persist_in_fresh_process(self):
        result, _ = self.run_case('selection-lost', enabled=True)
        self.assertEqual(result.returncode, 3)
        self.assertIn('selection was not confirmed', result.stderr)


if __name__ == '__main__':
    unittest.main()
