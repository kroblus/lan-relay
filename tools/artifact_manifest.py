"""Write a reproducible checksum manifest next to built application artifacts."""
import argparse
import hashlib
import json
from pathlib import Path


def manifest(folder: Path, source: str) -> None:
    if not folder.is_dir():
        raise SystemExit(f"Missing artifact directory: {folder}")
    files = []
    for path in sorted(folder.rglob('*')):
        if not path.is_file() or path.name in ('manifest.json', 'SHA256SUMS.txt'):
            continue
        digest = hashlib.sha256()
        with path.open('rb') as stream:
            while block := stream.read(1024 * 1024):
                digest.update(block)
        files.append({'path': path.relative_to(folder).as_posix(),
                      'bytes': path.stat().st_size, 'sha256': digest.hexdigest()})
    if not files:
        raise SystemExit('No application artifacts were produced')
    result = {'product': 'LAN Relay / 随手传', 'version': '0.4.0',
              'protocol_version': 2, 'source_commit': source, 'files': files}
    (folder / 'manifest.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    (folder / 'SHA256SUMS.txt').write_text(''.join(f"{item['sha256']}  {item['path']}\n" for item in files), encoding='utf-8')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('folder', type=Path)
    parser.add_argument('--source', required=True)
    args = parser.parse_args()
    manifest(args.folder, args.source)
