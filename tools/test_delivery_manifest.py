"""Acceptance regressions: the aggregate must match its actual platform files."""
import json
import tempfile
import unittest
import zipfile
from pathlib import Path

from artifact_manifest import manifest, product_version
from build_delivery import build_delivery, digest, verified_manifest


class DeliveryIntegrityTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = 'a' * 40
        self.version = product_version()
        self.windows = self.root / 'windows'
        self.android = self.root / 'android'
        for folder in [self.windows, self.android]:
            folder.mkdir()
        (self.windows / f'LanRelay-{self.version}-windows-x64.zip').write_bytes(b'fixture portable')
        (self.android / f'lan-relay-{self.version}-android-trial.apk').write_bytes(b'fixture apk, not installable')
        for folder in [self.windows, self.android]:
            manifest(folder, self.source, [path.name for path in folder.iterdir()])

    def test_bundle_contains_aggregate_and_exact_platform_bytes(self):
        output = self.root / 'delivery' / 'bundle'
        archive = build_delivery(self.windows, self.android, output, self.source, 123, 'fixture/relay')
        self.assertEqual((archive.parent / 'DELIVERY_MANIFEST.json').read_bytes(),
                         (output / 'DELIVERY_MANIFEST.json').read_bytes())
        with zipfile.ZipFile(archive) as bundle:
            self.assertIsNone(bundle.testzip())
            data = json.loads(bundle.read('DELIVERY_MANIFEST.json'))
            self.assertEqual(data['source_commit'], self.source)
            self.assertEqual(data['evidence_scope']['android_physical_device'], 'NOT RUN')
            for entry in data['delivery_files']:
                self.assertEqual(digest(output / entry['path']), entry['sha256'])
                self.assertEqual(bundle.read(entry['path']), (output / entry['path']).read_bytes())

    def test_tampered_file_is_rejected_before_output_creation(self):
        (self.android / f'lan-relay-{self.version}-android-trial.apk').write_bytes(b'wrong')
        output = self.root / 'delivery'
        with self.assertRaisesRegex(ValueError, 'checksum/size'):
            build_delivery(self.windows, self.android, output, self.source, 123, 'fixture/relay')
        self.assertFalse(output.exists())

    def test_mixed_source_commits_are_rejected(self):
        path = self.android / 'manifest.json'
        data = json.loads(path.read_text())
        data['source_commit'] = 'b' * 40
        path.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, 'source/version'):
            verified_manifest(self.android, self.source, self.version)

    def test_path_escape_is_rejected(self):
        path = self.windows / 'manifest.json'
        data = json.loads(path.read_text())
        data['files'][0]['path'] = '../outside'
        path.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, 'Unsafe'):
            verified_manifest(self.windows, self.source, self.version)

    def test_duplicate_manifest_entries_are_rejected(self):
        path = self.windows / 'manifest.json'
        data = json.loads(path.read_text())
        data['files'].append(data['files'][0])
        path.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, 'duplicate'):
            verified_manifest(self.windows, self.source, self.version)

    def test_existing_evidence_is_not_overwritten(self):
        output = self.root / 'delivery'
        output.mkdir()
        (output / 'evidence.txt').write_text('keep')
        with self.assertRaisesRegex(ValueError, 'existing evidence'):
            build_delivery(self.windows, self.android, output, self.source, 123, 'fixture/relay')
        self.assertEqual((output / 'evidence.txt').read_text(), 'keep')


if __name__ == '__main__':
    unittest.main()
