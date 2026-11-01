"""Collect the tested trial APK and the explicitly unsigned release APK."""
import argparse
import shutil
from pathlib import Path
from artifact_manifest import manifest

parser = argparse.ArgumentParser()
parser.add_argument('--source', required=True)
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
output = root / 'output/dist/android'
output.mkdir(parents=True, exist_ok=True)
for source, name in [
    ('apps/android/app/build/outputs/apk/debug/app-debug.apk', 'lan-relay-0.4.0-android-trial.apk'),
    ('apps/android/app/build/outputs/apk/release/app-release-unsigned.apk', 'lan-relay-0.4.0-android-release-unsigned.apk'),
]:
    path = root / source
    if not path.is_file():
        raise SystemExit(f'Missing APK: {path}')
    shutil.copy2(path, output / name)
(output / 'INSTALL.txt').write_text(
    '随手传 V0.4.0 / 协议 2\n\n'
    'android-trial.apk 是临时调试签名的可安装试用包。\n'
    'android-release-unsigned.apk 未签名，不能直接安装；须使用你保管的长期签名密钥签名。\n'
    '覆盖安装需要匹配 applicationId、签名和版本号。不要通过清除旧 App 数据掩盖签名冲突。\n'
    '自动构建通过不代表 Android 真机验收通过。\n', encoding='utf-8')
manifest(output, args.source)
