"""Write a reproducible checksum manifest next to built application artifacts."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import re
from pathlib import Path, PurePosixPath
import uuid


INPUTS_NAME = 'ARTIFACT_INPUTS.json'


def product_version() -> str:
    source = (Path(__file__).resolve().parents[1] / 'Cargo.toml').read_text(encoding='utf-8')
    match = re.search(r'^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"\s*$', source, re.MULTILINE)
    if not match:
        raise ValueError('Missing workspace product version')
    return match.group(1)


def _relative_path(value: str) -> str:
    pure = PurePosixPath(value)
    if (not value or pure.is_absolute() or '..' in pure.parts or '\\' in value
            or ':' in value or '\0' in value or pure.as_posix() != value):
        raise ValueError(f'Unsafe artifact path: {value!r}')
    return value


def publish_directory(staging: Path, destination: Path) -> Path | None:
    """Publish a fresh sibling directory, retaining earlier delivery evidence."""
    if (not staging.is_dir() or staging.is_symlink()
            or staging.parent.resolve() != destination.parent.resolve()):
        raise ValueError('Artifact staging must be a new sibling directory')
    if destination.is_symlink() or (destination.exists() and not destination.is_dir()):
        raise ValueError('Refusing to replace a non-directory artifact destination')
    previous = None
    if destination.exists():
        stamp = datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ')
        previous = destination.with_name(f'{destination.name}-previous-{stamp}-{uuid.uuid4().hex}')
        destination.rename(previous)
    try:
        staging.rename(destination)
    except BaseException:
        if previous is not None and not destination.exists():
            previous.rename(destination)
        raise
    return previous


def manifest(folder: Path, source: str, files: list[str] | None = None) -> None:
    if not folder.is_dir():
        raise SystemExit(f"Missing artifact directory: {folder}")
    if not re.fullmatch(r'[0-9a-f]{40}', source):
        raise ValueError('An exact source commit is required')
    if (folder / 'manifest.json').exists():
        raise ValueError('Existing artifact manifest is never relabelled; package into a fresh directory')
    if files is None:
        # Windows writes this only after completing a new isolated staging build.
        # Never turn every file left in a reused directory into current evidence.
        receipt = folder / INPUTS_NAME
        if not receipt.is_file() or receipt.is_symlink():
            raise ValueError('Missing explicit artifact inputs; run the platform packaging script first')
        inputs = json.loads(receipt.read_text(encoding='utf-8-sig'))
        if inputs.get('schema_version') != 1 or inputs.get('version') != product_version():
            raise ValueError('Artifact input version/schema mismatch')
        files = inputs.get('files')
        if not isinstance(files, list) or not files or not all(isinstance(v, str) for v in files):
            raise ValueError('Artifact inputs must list the files produced by this build')
        files = [*files, INPUTS_NAME]
    if not files or len(set(files)) != len(files):
        raise ValueError('Empty or duplicate artifact input list')
    expected = {_relative_path(value) for value in files}
    if expected & {'manifest.json', 'SHA256SUMS.txt'}:
        raise ValueError('Manifest metadata cannot be an artifact input')
    actual = set()
    for path in folder.rglob('*'):
        if path.is_symlink():
            raise ValueError('Symlinks are not accepted in artifact output')
        if path.is_file():
            relative = path.relative_to(folder).as_posix()
            if relative not in {'manifest.json', 'SHA256SUMS.txt'}:
                actual.add(relative)
    if actual != expected:
        raise ValueError(f'Unexpected or missing artifact files: extra={sorted(actual - expected)!r}, missing={sorted(expected - actual)!r}')
    entries = []
    for relative in sorted(expected):
        path = folder / relative
        if not path.resolve().is_relative_to(folder.resolve()):
            raise ValueError(f'Artifact escapes its directory: {relative!r}')
        digest = hashlib.sha256()
        with path.open('rb') as stream:
            while block := stream.read(1024 * 1024):
                digest.update(block)
        entries.append({'path': relative,
                      'bytes': path.stat().st_size, 'sha256': digest.hexdigest()})
    result = {'product': 'LAN Relay / 随手传', 'version': product_version(),
              'protocol_version': 2, 'source_commit': source, 'files': entries}
    with (folder / 'manifest.json').open('x', encoding='utf-8') as output:
        output.write(json.dumps(result, ensure_ascii=False, indent=2) + '\n')
    (folder / 'SHA256SUMS.txt').write_text(''.join(f"{item['sha256']}  {item['path']}\n" for item in entries), encoding='utf-8')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('folder', type=Path)
    parser.add_argument('--source', required=True)
    args = parser.parse_args()
    manifest(args.folder, args.source)
