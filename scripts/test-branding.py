#!/usr/bin/env python3
"""Verify private bundle identity overrides without touching installed applications."""
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent


class BrandingTest(unittest.TestCase):
    def test_public_and_private_identity_agree_across_builders(self):
        with tempfile.TemporaryDirectory(prefix='openfig-branding-') as tmp:
            root = Path(tmp)
            for name in ['branding.env', 'scripts/build-input-method.sh',
                         'crates/fig_util/build.rs', 'native/input-method/Info.plist',
                         'native/input-method/main.m', 'bundle/macos/AppIcon.icns']:
                target = root / name
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(ROOT / name, target)
            # Use the actual Justfile identity expression, without invoking any app recipe.
            expression = next(line for line in (ROOT / 'Justfile').read_text().splitlines()
                              if line.startswith('export BUNDLE_ID :='))
            (root / 'Justfile').write_text('set dotenv-load := true\nset dotenv-override := true\nset dotenv-filename := "branding.env"\n' + expression + '\nprobe:\n    echo "$BUNDLE_ID"\n')
            binary = root / 'branding-build'
            subprocess.run(['rustc', '--edition=2024', '-Dwarnings', 'crates/fig_util/build.rs', '-o', str(binary)], cwd=root, check=True)
            for identity in ['local.openfig', 'local.example.quoted', 'local.example.existing']:
                if identity == 'local.example.quoted':
                    path = root / 'branding.env'
                    path.write_text(path.read_text().replace('BUNDLE_ID=local.openfig', 'export BUNDLE_ID = \"local.example.quoted\" # public override'))
                elif identity == 'local.example.existing':
                    (root / 'bundle-id.local').write_text(identity + '\n')
                just = subprocess.check_output(['just', '--evaluate', 'BUNDLE_ID'], cwd=root, text=True)
                self.assertEqual(just.strip(), identity)
                rust = subprocess.check_output([str(binary)], cwd=root / 'crates/fig_util', text=True)
                self.assertIn('cargo:rustc-env=FIG_BRAND_BUNDLE_ID=' + identity + '\n', rust)
                app = root / 'build/FigInputMethod.app'
                subprocess.run(['bash', 'scripts/build-input-method.sh', str(app)], cwd=root, check=True,
                               stdout=subprocess.DEVNULL)
                info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
                self.assertEqual(info['CFBundleIdentifier'], identity + '.inputmethod')
                self.assertEqual(info['TISInputSourceID'], identity + '.inputmethod')
                self.assertEqual(info['InputMethodConnectionName'], identity + '.inputmethod.connection')
                # Only reads the keyboard source; never registers, selects or launches an IMK server.
                result = subprocess.run([str(app / 'Contents/MacOS/fig_input_method'), '--keyboard-source'],
                                        capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
