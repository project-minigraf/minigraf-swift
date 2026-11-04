#!/usr/bin/env bash
# Disposable source checkouts only. GitHub builds and checks correctness;
# measurements are collected separately on physical Device Farm hardware.
set -euo pipefail
binding_dir=$(cd "$(dirname "$0")/../.." && pwd)
core_dir=$(cd "${1:?usage: build.sh CORE_CHECKOUT}" && pwd)
cd "$binding_dir"
core_revision=$(git -C "$core_dir" rev-parse HEAD)
binding_revision=$(git rev-parse HEAD)
test ! -e .cargo/config.toml
test ! -e bench/device-farm/build
mkdir -p .cargo bench/device-farm/build/evidence
python3 - "$core_dir" <<'PY'
import json, pathlib, sys
pathlib.Path('.cargo/config.toml').write_text(
    '[patch."https://github.com/project-minigraf/minigraf"]\n'
    'minigraf = { path = ' + json.dumps(sys.argv[1]) + ' }\n')
PY
cargo update -p minigraf
cargo metadata --format-version 1 > bench/device-farm/build/evidence/cargo-metadata.json
python3 - "$core_dir" <<'PY'
import json, pathlib, sys
p = [p for p in json.load(open('bench/device-farm/build/evidence/cargo-metadata.json'))['packages'] if p['name'] == 'minigraf']
assert len(p) == 1 and p[0]['source'] is None
assert pathlib.Path(p[0]['manifest_path']).parent == pathlib.Path(sys.argv[1])
PY
cargo test --lib
cargo build --locked --release --lib --target aarch64-apple-ios
cargo build --locked --release --lib --target aarch64-apple-ios-sim
cargo run --locked --release --bin uniffi-bindgen -- generate \
  --library target/aarch64-apple-ios/release/libminigraf_ffi.a \
  --language swift --out-dir bench/device-farm/build/generated
mkdir bench/device-farm/build/includes
cp bench/device-farm/build/generated/*.h bench/device-farm/build/includes/
cp bench/device-farm/build/generated/minigraf_ffiFFI.modulemap bench/device-farm/build/includes/module.modulemap
xcodebuild -create-xcframework \
  -library target/aarch64-apple-ios/release/libminigraf_ffi.a -headers bench/device-farm/build/includes \
  -library target/aarch64-apple-ios-sim/release/libminigraf_ffi.a -headers bench/device-farm/build/includes \
  -output bench/device-farm/build/minigrafFFI.xcframework
python3 - "$core_revision" "$binding_revision" <<'PY'
import hashlib, json, pathlib, subprocess, sys
root = pathlib.Path('bench/device-farm')
digest = hashlib.sha256()
for p in sorted(p for p in root.rglob('*') if p.is_file() and
                not any(part in ('build', 'MinigrafBenchmark.xcodeproj') for part in p.relative_to(root).parts)):
    digest.update(str(p.relative_to(root)).encode() + b'\0' + p.read_bytes() + b'\0')
def sha(p): return hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()
packages = json.load(open(root/'build/evidence/cargo-metadata.json'))['packages']
uniffi = [p['version'] for p in packages if p['name'] == 'uniffi']
assert len(uniffi) == 1
record = dict(core_revision=sys.argv[1], binding_revision=sys.argv[2], evidence_kind='source-build',
    harness_sha256=digest.hexdigest(), cargo_lock_sha256=sha('Cargo.lock'),
    native_sha256=sha('target/aarch64-apple-ios/release/libminigraf_ffi.a'),
    swift_binding_sha256=sha(root/'build/generated/minigraf_ffi.swift'),
    rustc=subprocess.check_output(['rustc','--version'],text=True).strip(),
    xcode=subprocess.check_output(['xcodebuild','-version'],text=True).strip(),
    ios_sdk=subprocess.check_output(['xcrun','--sdk','iphoneos','--show-sdk-version'],text=True).strip(),
    profile='Release', swift_optimization='-O', rust_profile='release (binding workspace defaults)',
    abi='aarch64-apple-ios', uniffi=uniffi[0], xcodegen='2.44.1')
(root/'build/build.json').write_text(json.dumps(record,indent=2)+'\n')
PY
cp Cargo.lock bench/device-farm/build/evidence/
cd bench/device-farm
curl --fail --location --retry 3 https://github.com/yonaskolb/XcodeGen/releases/download/2.44.1/xcodegen.zip -o build/xcodegen.zip
echo 'a2e905fb68446e9bb4008cdfe2e13e3f176d0cbcca828b71770f8e53fca91b73  build/xcodegen.zip' | shasum -a 256 -c -
unzip -q build/xcodegen.zip -d build/tools
build/tools/xcodegen/bin/xcodegen generate
simulator=$(xcrun simctl list devices available --json | python3 -c '
import json,sys
devices=json.load(sys.stdin)["devices"]
ios=sorted((k for k in devices if "iOS" in k), reverse=True)
print(next(d["udid"] for k in ios for d in devices[k] if d["name"].startswith("iPhone")))')
# Explicit allowlist: no benchmark measurements on GitHub runners.
xcodebuild test -project MinigrafBenchmark.xcodeproj -scheme MinigrafBenchmark \
  -configuration Release -destination "id=$simulator" -derivedDataPath build/simulator \
  -resultBundlePath build/evidence/correctness.xcresult \
  -only-testing:BenchmarkTests/BenchmarkTests/testOracleRejectsEmptyAndWrongRows \
  -only-testing:BenchmarkTests/BenchmarkTests/testSmoke \
  CODE_SIGNING_ALLOWED=NO ONLY_ACTIVE_ARCH=YES | tee build/evidence/correctness.log
xcodebuild build-for-testing -project MinigrafBenchmark.xcodeproj -scheme MinigrafBenchmark \
  -configuration Release -destination 'generic/platform=iOS' -derivedDataPath build/device \
  CODE_SIGNING_ALLOWED=NO | tee build/evidence/device-build.log
mkdir -p build/package/Payload build/dist
cp -R build/device/Build/Products/Release-iphoneos/MinigrafBenchmark.app build/package/Payload/
(cd build/package && zip -qr ../dist/MinigrafBenchmark.ipa Payload)
(cd build/package/Payload/MinigrafBenchmark.app/PlugIns && zip -qr ../../../../dist/BenchmarkTests.xctest.zip BenchmarkTests.xctest)
cp build/build.json build/dist/
cp build/evidence/Cargo.lock build/dist/
(cd build/dist && shasum -a 256 MinigrafBenchmark.ipa BenchmarkTests.xctest.zip build.json Cargo.lock > SHA256SUMS)
