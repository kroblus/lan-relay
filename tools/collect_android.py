"""Collect the tested trial APK and the explicitly unsigned release APK."""
import argparse
import json
import shutil
import tempfile
from pathlib import Path
from artifact_manifest import manifest, product_version, publish_directory


def collect_android(root: Path, source_commit: str) -> tuple[Path, Path | None]:
    version = product_version()
    output = root / f'output/dist/v{version}/android'
    products = [
        ('apps/android/app/build/outputs/apk/debug/app-debug.apk', f'lan-relay-{version}-android-trial.apk'),
        ('apps/android/app/build/outputs/apk/release/app-release-unsigned.apk', f'lan-relay-{version}-android-release-unsigned.apk'),
    ]
    for source, _ in products:
        if not (root / source).is_file():
            raise ValueError(f'Missing APK: {root / source}')
    output.parent.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix='.android-staging-', dir=output.parent))
    # Failure preserves the previous output and leaves staging for diagnosis.
    for source, name in products:
        shutil.copy2(root / source, staging / name)
    (staging / 'INSTALL.txt').write_text(
        f'随手传 V{version} / 协议 2\n\n'
        'android-trial.apk 是临时调试签名的可安装试用包。\n'
        'android-release-unsigned.apk 未签名，不能直接安装；须使用你保管的长期签名密钥签名。\n'
        '覆盖安装需要匹配 applicationId、签名和版本号。不要通过清除旧 App 数据掩盖签名冲突。\n'
        '自动构建通过不代表 Android 真机验收通过。\n', encoding='utf-8')
    manifest(staging, source_commit, [name for _, name in products] + ['INSTALL.txt'])
    previous = publish_directory(staging, output)
    return output, previous


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--source', required=True)
    args = parser.parse_args()
    output, previous = collect_android(Path(__file__).resolve().parents[1], args.source)
    print(json.dumps({'output': str(output), 'previous_preserved': str(previous) if previous else None}))
