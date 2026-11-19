"""Exercise the release boundary without a certificate, Apple submission or installed application."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

HERE = Path(__file__).resolve().parent


class ReleaseTest(unittest.TestCase):
    def test_public_checkout_and_ticket_survive_zip_roundtrip(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = root / 'repo'
            (repo / 'scripts').mkdir(parents=True)
            shutil.copy2(HERE / 'check-publication.py', repo / 'scripts')
            (repo / 'branding.env').write_text('APP_NAME=Fig\nBUNDLE_ID=local.openfig\n')
            (repo / 'bundle-id.local').write_text('private.id\n')
            subprocess.run(['git', 'init', '-q'], cwd=repo, check=True)
            subprocess.run(['git', 'add', 'branding.env', 'scripts'], cwd=repo, check=True)
            subprocess.run(['git', '-c', 'user.name=Fixture', '-c', 'user.email=test@example.invalid',
                            'commit', '-qm', 'fixture'], cwd=repo, check=True)
            bin_dir = root / 'bin'
            bin_dir.mkdir()
            commands = {
                'security': """echo '1) 0001 "Developer ID Application: Fixture"'""",
                'just': '''test ! -e bundle-id.local
test "$FIG_DISTRIBUTION_SIGNING" = 1
mkdir -p build/Fig.app/Contents/Resources/themes
plist=build/Fig.app/Contents/Info.plist
plutil -create xml1 "$plist"
plutil -insert CFBundleIdentifier -string local.openfig "$plist"''',
                'codesign': 'test "$1" = --verify',
                'xcrun': '''case "$1 $2" in
  "notarytool submit") echo "{\\"status\\":\\"$VERDICT\\"}" ;;
  "stapler staple") touch "$3/ticket" ;;
  "stapler validate") test -f "$3/ticket" ;;
  *) exit 1 ;;
esac''',
                'spctl': 'test -f "${!#}/ticket"',
            }
            for name, command in commands.items():
                program = bin_dir / name
                program.write_text('#!/bin/bash\nset -eu\n' + command + '\n')
                program.chmod(0o700)
            env = dict(os.environ, PATH=f'{bin_dir}:{os.environ["PATH"]}',
                       FIG_SIGN_IDENTITY='Developer ID Application: Fixture', FIG_NOTARY_PROFILE='fixture')
            def run(verdict, **extra):
                return subprocess.run(['/bin/bash', HERE / 'release-macos.sh'], cwd=repo,
                                      env=dict(env, VERDICT=verdict, **extra), capture_output=True, text=True)
            # The installed certificate, approval and final archive checks are independent.
            self.assertNotEqual(run('Accepted', FIG_SIGN_IDENTITY='-').returncode, 0)
            self.assertNotEqual(run('Invalid').returncode, 0)
            self.assertFalse((repo / 'build/releases').exists())
            result = run('Accepted')
            self.assertEqual(result.returncode, 0, result.stderr)
            checksum, = (repo / 'build/releases').glob('*/SHA256SUMS')
            subprocess.run(['shasum', '-a', '256', '-c', checksum.name], cwd=checksum.parent, check=True)
            self.assertNotEqual(run('Accepted').returncode, 0)


if __name__ == '__main__':
    unittest.main()
