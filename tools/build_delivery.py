"""Verify CI platform artifacts and create one discoverable delivery bundle.

Run only after the platform jobs succeeded. This records build evidence, never
claims physical-device acceptance, and refuses mismatched or tampered inputs.
"""
import argparse
import hashlib
import json
import re
import shutil
import zipfile
from pathlib import Path, PurePosixPath

from artifact_manifest import product_version


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open('rb') as stream:
        while block := stream.read(1024 * 1024):
            value.update(block)
    return value.hexdigest()


def verified_manifest(folder: Path, source: str, version: str) -> dict:
    data = json.loads((folder / 'manifest.json').read_text(encoding='utf-8'))
    if data.get('source_commit') != source or data.get('version') != version:
        raise ValueError(f'Artifact source/version mismatch: {folder}')
    if data.get('protocol_version') != 2 or not data.get('files'):
        raise ValueError(f'Invalid artifact manifest: {folder}')
    seen = set()
    for entry in data['files']:
        relative = entry['path']
        pure = PurePosixPath(relative)
        if (not relative or pure.is_absolute() or '..' in pure.parts
                or '\\' in relative or ':' in relative or relative in seen):
            raise ValueError(f'Unsafe or duplicate artifact path: {relative!r}')
        seen.add(relative)
        path = folder / relative
        if not path.resolve().is_relative_to(folder.resolve()) or path.is_symlink():
            raise ValueError(f'Artifact escapes its directory: {relative!r}')
        if not path.is_file() or path.stat().st_size != entry['bytes'] or digest(path) != entry['sha256']:
            raise ValueError(f'Artifact checksum/size mismatch: {relative!r}')
    return data


def build_delivery(windows: Path, android: Path, output: Path,
                   source: str, run_id: int, repository: str) -> Path:
    if not re.fullmatch(r'[0-9a-f]{40}', source) or run_id <= 0:
        raise ValueError('An exact source commit and positive workflow run are required')
    if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repository):
        raise ValueError('Invalid repository name')
    version = product_version()
    manifests = {
        'windows': verified_manifest(windows, source, version),
        'android': verified_manifest(android, source, version),
    }
    if output.exists():
        raise ValueError('Delivery output must be new; existing evidence is never overwritten')
    selections = {
        'windows': [f'LanRelay-{version}-windows-x64.zip'],
        'android': [f'lan-relay-{version}-android-trial.apk'],
    }
    installer = f'LanRelay-{version}-windows-setup.exe'
    windows_files = {entry['path'] for entry in manifests['windows']['files']}
    if installer in windows_files:
        selections['windows'].append(installer)
    for platform, names in selections.items():
        available = {entry['path'] for entry in manifests[platform]['files']}
        if not set(names).issubset(available):
            raise ValueError(f'Missing installable {platform} delivery artifacts')
    output.mkdir(parents=True)
    delivery_files = []
    for platform, folder in [('windows', windows), ('android', android)]:
        destination = output / platform
        destination.mkdir()
        for name in selections[platform]:
            target = destination / name
            shutil.copy2(folder / name, target)
            delivery_files.append({'path': f'{platform}/{name}',
                                   'bytes': target.stat().st_size, 'sha256': digest(target)})
        shutil.copy2(folder / 'manifest.json', destination / 'manifest.json')
    manifest = {
        'schema_version': 1,
        'product': 'LAN Relay / 随手传', 'version': version, 'protocol_version': 2,
        'source_commit': source, 'workflow_run': run_id,
        'workflow_url': f'https://github.com/{repository}/actions/runs/{run_id}',
        'ci_artifacts': manifests,
        'delivery_files': delivery_files,
        'evidence_scope': {
            'automated_ci_gates': 'PASS',
            'windows_native_gui_on_user_device': 'NOT RUN',
            'android_physical_device': 'NOT RUN',
            'real_lan_discovery_and_wifi_performance': 'NOT RUN',
        },
        'signing': {
            'android_trial': 'CI debug signing; compare the actual certificate before upgrading',
            'android_release': 'Unsigned release stays in the platform CI artifact, not this installable bundle',
        },
    }
    (output / 'DELIVERY_MANIFEST.json').write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    (output / 'README.md').write_text(
        f'# 随手传 {version} 试用交付\n\n'
        f'源码：`{source}`。CI：{manifest["workflow_url"]}\n\n'
        '`DELIVERY_MANIFEST.json` 位于本压缩包根目录，并随 delivery CI artifact 提供。\n\n'
        'Windows 安装包/便携包位于 windows/，Android 可安装试用包位于 android/。\n'
        '两个平台的原始 manifest.json 同时保留；先核对实际文件 SHA-256。\n'
        '试用 APK 的签名可能与旧 CI 试用包不同。无法覆盖安装时保留旧 App 数据，'
        '使用原签名构建，或明确选择独立试用；不要自动卸载。\n\n'
        '两端打开应用后从「设备」发现同一局域网设备，首次核对后批准。'
        '普通收发无需填写开发验收记录；安装 APK 仍由系统要求本人确认。\n\n'
        '自动构建和单测通过不代表 Windows 原生界面、Android 真机或真实 Wi-Fi 已通过。'
        '请分别记录 Windows 与 Android 真机测试结果；编译通过不代表真机验收通过。\n', encoding='utf-8')
    hashes = []
    for path in sorted(output.rglob('*')):
        if path.is_file():
            hashes.append(f'{digest(path)}  {path.relative_to(output).as_posix()}\n')
    (output / 'SHA256SUMS.txt').write_text(''.join(hashes), encoding='utf-8')
    archive = output.parent / f'LanRelay-{version}-delivery.zip'
    if archive.exists():
        raise ValueError('Delivery archive already exists; do not overwrite evidence')
    with zipfile.ZipFile(archive, 'w', compression=zipfile.ZIP_DEFLATED, compresslevel=6) as bundle:
        for path in sorted(output.rglob('*')):
            if path.is_file():
                bundle.write(path, path.relative_to(output).as_posix())
    # Also expose the aggregate directly at the CI artifact root, so reviewers
    # do not have to locate a separately hosted file or inspect nested ZIPs.
    aggregate = archive.parent / 'DELIVERY_MANIFEST.json'
    shutil.copy2(output / 'DELIVERY_MANIFEST.json', aggregate)
    (archive.parent / 'DELIVERY_SHA256SUMS.txt').write_text(
        f'{digest(aggregate)}  {aggregate.name}\n{digest(archive)}  {archive.name}\n',
        encoding='utf-8')
    return archive


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--windows', type=Path, required=True)
    parser.add_argument('--android', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--source', required=True)
    parser.add_argument('--run-id', type=int, required=True)
    parser.add_argument('--repository', required=True)
    args = parser.parse_args()
    archive = build_delivery(args.windows, args.android, args.output,
                             args.source, args.run_id, args.repository)
    print(json.dumps({'delivery_zip': str(archive), 'bytes': archive.stat().st_size,
                      'sha256': digest(archive),
                      'manifest': str(args.output / 'DELIVERY_MANIFEST.json')}))
