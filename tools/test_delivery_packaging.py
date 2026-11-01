"""Fresh packaging must not relabel earlier or unexpected application files."""
import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from artifact_manifest import INPUTS_NAME, manifest, product_version, publish_directory
from collect_android import collect_android


class PackagingFreshnessTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = 'a' * 40
        self.version = product_version()

    def inputs(self, folder, files):
        (folder / INPUTS_NAME).write_text(json.dumps({
            'schema_version': 1, 'version': self.version, 'files': files,
        }), encoding='utf-8')

    def test_old_installer_is_rejected_when_not_produced_by_this_build(self):
        folder = self.root / 'windows'
        folder.mkdir()
        portable = f'LanRelay-{self.version}-windows-x64.zip'
        (folder / portable).write_bytes(b'current portable fixture')
        installer = folder / f'LanRelay-{self.version}-windows-setup.exe'
        installer.write_bytes(b'previous installer fixture')
        self.inputs(folder, [portable])
        with self.assertRaisesRegex(ValueError, 'Unexpected'):
            manifest(folder, self.source)
        self.assertFalse((folder / 'manifest.json').exists())
        self.assertEqual(installer.read_bytes(), b'previous installer fixture')

    def test_unexpected_file_is_rejected_with_explicit_products(self):
        folder = self.root / 'android'
        folder.mkdir()
        (folder / 'current.apk').write_bytes(b'test fixture, not installable')
        (folder / 'leftover.log').write_text('preserve this evidence')
        with self.assertRaisesRegex(ValueError, 'Unexpected'):
            manifest(folder, self.source, ['current.apk'])
        self.assertFalse((folder / 'manifest.json').exists())
        self.assertEqual((folder / 'leftover.log').read_text(), 'preserve this evidence')

    def test_missing_explicit_receipt_cannot_relabel_reused_directory(self):
        (self.root / 'old-setup.exe').write_bytes(b'old binary fixture')
        with self.assertRaisesRegex(ValueError, 'explicit artifact inputs'):
            manifest(self.root, self.source)

    def test_existing_manifest_cannot_be_relabelled_to_another_commit(self):
        (self.root / 'current.apk').write_bytes(b'fixture')
        manifest(self.root, self.source, ['current.apk'])
        before = (self.root / 'manifest.json').read_bytes()
        with self.assertRaisesRegex(ValueError, 'never relabelled'):
            manifest(self.root, 'b' * 40, ['current.apk'])
        self.assertEqual((self.root / 'manifest.json').read_bytes(), before)

    def test_fresh_publication_retains_old_directory_without_mixing(self):
        destination = self.root / 'windows'
        destination.mkdir()
        (destination / 'old-setup.exe').write_bytes(b'previous installer')
        staging = self.root / '.windows-staging-fixture'
        staging.mkdir()
        (staging / 'new-portable.zip').write_bytes(b'new portable')
        previous = publish_directory(staging, destination)
        self.assertIsNotNone(previous)
        self.assertEqual((previous / 'old-setup.exe').read_bytes(), b'previous installer')
        self.assertEqual({p.name for p in destination.iterdir()}, {'new-portable.zip'})
        self.assertFalse(staging.exists())

    def test_failed_publication_restores_prior_output_and_keeps_staging(self):
        destination = self.root / 'android'
        destination.mkdir()
        (destination / 'old.apk').write_bytes(b'keep old')
        staging = self.root / '.android-staging-fixture'
        staging.mkdir()
        (staging / 'new.apk').write_bytes(b'keep new staging')
        rename = Path.rename

        def fail_staging(path, target):
            if path == staging:
                raise OSError('simulated publication failure')
            return rename(path, target)

        with patch.object(Path, 'rename', fail_staging):
            with self.assertRaisesRegex(OSError, 'publication failure'):
                publish_directory(staging, destination)
        self.assertEqual((destination / 'old.apk').read_bytes(), b'keep old')
        self.assertEqual((staging / 'new.apk').read_bytes(), b'keep new staging')

    def android_sources(self):
        for relative, data in [
            ('debug/app-debug.apk', b'current debug fixture, not installable'),
            ('release/app-release-unsigned.apk', b'current unsigned fixture'),
        ]:
            path = self.root / 'apps/android/app/build/outputs/apk' / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)

    def test_android_recollection_preserves_old_files_outside_current_manifest(self):
        self.android_sources()
        output = self.root / f'output/dist/v{self.version}/android'
        output.mkdir(parents=True)
        (output / 'leftover.apk').write_bytes(b'old package, retain')
        (output / 'manifest.json').write_text('{"source_commit":"old"}')
        current, previous = collect_android(self.root, self.source)
        self.assertEqual(current, output)
        self.assertEqual((previous / 'leftover.apk').read_bytes(), b'old package, retain')
        self.assertFalse((output / 'leftover.apk').exists())
        data = json.loads((output / 'manifest.json').read_text())
        self.assertEqual(data['source_commit'], self.source)
        self.assertEqual({entry['path'] for entry in data['files']}, {
            f'lan-relay-{self.version}-android-trial.apk',
            f'lan-relay-{self.version}-android-release-unsigned.apk', 'INSTALL.txt',
        })

    def test_missing_new_apk_preserves_existing_android_output(self):
        self.android_sources()
        (self.root / 'apps/android/app/build/outputs/apk/release/app-release-unsigned.apk').unlink()
        output = self.root / f'output/dist/v{self.version}/android'
        output.mkdir(parents=True)
        (output / 'old.apk').write_bytes(b'keep old')
        with self.assertRaisesRegex(ValueError, 'Missing APK'):
            collect_android(self.root, self.source)
        self.assertEqual((output / 'old.apk').read_bytes(), b'keep old')
        self.assertEqual(len(list(output.parent.iterdir())), 1)

    @unittest.skipUnless(shutil.which('pwsh'), 'PowerShell runtime is unavailable')
    def test_actual_windows_skipbuild_skipinstaller_excludes_previous_setup(self):
        project = self.root / 'project'
        packaging = project / 'packaging/windows'
        packaging.mkdir(parents=True)
        script = Path(__file__).resolve().parents[1] / 'packaging/windows/Build.ps1'
        shutil.copy2(script, packaging / 'Build.ps1')
        (packaging / 'QUICK_START.txt').write_text('fixture guide')
        target = project / 'target/release'
        target.mkdir(parents=True)
        for name in ['lan-relay.exe', 'relay-server.exe', 'relay-migrate.exe']:
            (target / name).write_bytes(b'non-executable packaging fixture')
        output = project / f'output/dist/v{self.version}/windows'
        output.mkdir(parents=True)
        installer = f'LanRelay-{self.version}-windows-setup.exe'
        (output / installer).write_bytes(b'previous installer fixture')
        (output / 'unexpected.txt').write_text('old evidence')
        subprocess.run([
            shutil.which('pwsh'), '-NoProfile', '-File', str(packaging / 'Build.ps1'),
            '-SkipBuild', '-SkipInstaller',
        ], check=True, capture_output=True, timeout=30)
        self.assertFalse((output / installer).exists())
        self.assertFalse((output / 'unexpected.txt').exists())
        previous = list(output.parent.glob('windows-previous-*'))
        self.assertEqual(len(previous), 1)
        self.assertEqual((previous[0] / installer).read_bytes(), b'previous installer fixture')
        manifest(output, self.source)
        data = json.loads((output / 'manifest.json').read_text())
        self.assertNotIn(installer, {entry['path'] for entry in data['files']})


if __name__ == '__main__':
    unittest.main()
