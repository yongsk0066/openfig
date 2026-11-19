#!/usr/bin/env python3
"""App replacement fault injection; never touches Applications or real processes."""
from pathlib import Path
import os, shutil, subprocess, tempfile, unittest

INSTALLER = Path(__file__).with_name('install-app.sh').resolve()
class BashTests(unittest.TestCase):
    shell = '/bin/bash'

    def shortDescription(self):
        return self.shell

class AppInstallTests(BashTests):
    def run_case(self, failure):
        with tempfile.TemporaryDirectory(prefix='fig install ') as directory:
            root = Path(directory)
            source, destination = root/'new.app', root/'Fig.app'
            for app, marker in [(source, 'new'), (destination, 'old')]:
                for relative in ['Resources/install-input-method.sh', 'MacOS/q', 'MacOS/fig_desktop', 'MacOS/fig_native_worker', 'MacOS/fig_io_worker', 'Helpers/FigInputMethod.app/Contents/MacOS/fig_input_method']:
                    binary = app/'Contents'/relative
                    binary.parent.mkdir(parents=True, exist_ok=True)
                    binary.write_text('#!/bin/sh\nexit 0\n'); binary.chmod(0o700)
                (app/'marker').write_text(marker)
            if failure in ['changed-ime', 'resigned-ime']:
                helper = source/'Contents/Helpers/FigInputMethod.app/Contents/MacOS/fig_input_method'
                helper.write_text('#!/bin/sh\necho "helper $1"\nexit 0\n')
            if failure in ['helper', 'io-helper']: (source/'Contents/MacOS'/('fig_io_worker' if failure == 'io-helper' else 'fig_native_worker')).unlink()
            code = '''source "$1"
ditto() { [[ "$FAILURE" != copy ]] || return 1; cp -R "$1" "$2"; }
codesign() {
 [[ "$FAILURE" != signature ]] || return 1
 if [[ "$1" == -d && "$FAILURE" == resigned-ime ]]; then echo CDHash=same-code >&2; fi
 return 0
}
pgrep() { [[ "$FAILURE" == stop ]]; }
sleep() { :; }
open() {
 if [[ "$FAILURE" == launch && ! -e "$TEST_ROOT/launch-failed" ]]; then
   touch "$TEST_ROOT/launch-failed"; return 1
 fi
 return 0
}
replace_app "$2" "$3" q fig_desktop
'''
            result = subprocess.run([self.shell,'-c',code,'test',str(INSTALLER),str(source),str(destination)],
                                    env=dict(os.environ,FAILURE=failure,TEST_ROOT=directory),capture_output=True,text=True,timeout=10)
            self.assertEqual(list(root.glob('.fig-install.*')), [], result.stderr)
            return result, (destination/'marker').read_text(), [p.joinpath('marker').read_text() for p in root.glob('.fig-backups/Fig-*.app')]
    def test_success_retains_previous_bundle(self):
        result, current, backups = self.run_case('none')
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual((current,backups),('new',['old']))
    def test_only_changed_input_method_is_restarted(self):
        result, current, _ = self.run_case('changed-ime')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(current, 'new')
        self.assertIn('helper --stop-running', result.stdout)
        self.assertIn('approved terminal clients reconnect automatically', result.stdout)
        result, _, _ = self.run_case('none')
        self.assertNotIn('approved terminal clients reconnect automatically', result.stdout)

    def test_resigning_unchanged_code_preserves_input_clients(self):
        result, current, _ = self.run_case('resigned-ime')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(current, 'new')
        self.assertNotIn('helper --stop-running', result.stdout)
        self.assertNotIn('approved terminal clients reconnect automatically', result.stdout)

    def test_copy_failure_preserves_installed_app(self):
        result, current, _ = self.run_case('copy')
        self.assertNotEqual(result.returncode,0); self.assertEqual(current,'old')
    def test_missing_native_helper_preserves_installed_app(self):
        result, current, _ = self.run_case('helper')
        self.assertNotEqual(result.returncode,0); self.assertEqual(current,'old')
    def test_missing_io_helper_preserves_installed_app(self):
        result, current, _ = self.run_case('io-helper')
        self.assertNotEqual(result.returncode,0); self.assertEqual(current,'old')
    def test_invalid_signature_preserves_installed_app(self):
        result, current, _ = self.run_case('signature')
        self.assertNotEqual(result.returncode,0); self.assertEqual(current,'old')
    def test_live_process_prevents_replacement(self):
        result, current, _ = self.run_case('stop')
        self.assertNotEqual(result.returncode,0); self.assertEqual(current,'old')
    def test_launch_failure_restores_previous_bundle(self):
        result, current, _ = self.run_case('launch')
        self.assertNotEqual(result.returncode,0); self.assertEqual(current,'old')
        self.assertIn('Restored previous app',result.stderr)

class AppStageTests(BashTests):
    def test_stage_never_stops_launches_or_replaces_live_app(self):
        for failure in ['none', 'copy', 'signature', 'helper', 'io-helper']:
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source, destination, live = root/'new.app', root/'updates/pending.app', root/'Fig.app'
                live.mkdir(); (live/'marker').write_text('running-old')
                for relative in ['Resources/install-input-method.sh', 'MacOS/q', 'MacOS/fig_desktop', 'MacOS/fig_native_worker', 'MacOS/fig_io_worker', 'Helpers/FigInputMethod.app/Contents/MacOS/fig_input_method']:
                    binary = source/'Contents'/relative
                    binary.parent.mkdir(parents=True, exist_ok=True)
                    binary.write_text('#!/bin/sh\nexit 99\n'); binary.chmod(0o700)
                if failure in ['helper', 'io-helper']: (source/'Contents/MacOS'/('fig_io_worker' if failure == 'io-helper' else 'fig_native_worker')).unlink()
                code = '''source "$1"
                ditto() { [[ "$FAILURE" != copy ]] || return 1; cp -R "$1" "$2"; }
                codesign() { [[ "$FAILURE" != signature ]]; }
                open() { touch "$TEST_ROOT/unexpected"; return 99; }
                pgrep() { touch "$TEST_ROOT/unexpected"; return 99; }
                stage_app "$2" "$3" q fig_desktop
                '''
                result = subprocess.run([self.shell,'-c',code,'test',str(INSTALLER),str(source),str(destination)],
                    env=dict(os.environ, FAILURE=failure, TEST_ROOT=directory), capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode == 0, failure == 'none', result.stderr)
                self.assertEqual(destination.exists(), failure == 'none')
                self.assertEqual((live/'marker').read_text(), 'running-old')
                self.assertFalse((root/'unexpected').exists())
                self.assertEqual(list((root/'updates').glob('.fig-stage.*')), [])

def load_tests(loader, tests, pattern):
    suite = unittest.TestSuite()
    shells = {str(Path('/bin/bash').resolve()), str(Path(shutil.which('bash') or '/bin/bash').resolve())}
    for shell in sorted(shells):
        for case in (AppInstallTests, AppStageTests):
            for test in loader.loadTestsFromTestCase(case):
                test.shell = shell
                suite.addTest(test)
    return suite

if __name__ == '__main__': unittest.main()
