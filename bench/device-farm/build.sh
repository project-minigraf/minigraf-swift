#!/usr/bin/env bash
# Disposable source checkouts only. GitHub builds and checks correctness;
# #839 measurements remain physical-only. Qualification is explicitly opt-in;
# this script itself always runs correctness only, even in qualification mode.
set -euo pipefail
binding_dir=$(cd "$(dirname "$0")/../.." && pwd)
core_dir=$(cd "${1:?usage: build.sh CORE_CHECKOUT CORE_SHA [qualification SELECTION_JSON]}" && pwd)
pinned_core=${2:?a full pinned core SHA is required}
build_mode=${3:-correctness}
selection=${4:-}
[[ "$pinned_core" =~ ^[0-9a-f]{40}$ ]] || { echo 'core SHA must be 40 lowercase hex characters' >&2; exit 1; }
[[ "$build_mode" == correctness || "$build_mode" == qualification ]] || exit 1
if [[ "$build_mode" == qualification ]]; then
  selection=$(cd "$(dirname "${selection:?qualification requires SELECTION_JSON}")" && pwd)/$(basename "$selection")
fi
cd "$binding_dir"
core_revision=$(git -C "$core_dir" rev-parse HEAD)
[[ "$core_revision" == "$pinned_core" ]] || { echo 'core checkout does not match pinned SHA' >&2; exit 1; }
# A label on modified sources cannot establish matched builds.
test -z "$(git -C "$core_dir" status --porcelain)"
test -z "$(git status --porcelain)"
binding_revision=$(git rev-parse HEAD)
sim_arch=$(uname -m)
[[ "$sim_arch" == arm64 || "$sim_arch" == x86_64 ]] || { echo 'unsupported simulator architecture' >&2; exit 1; }
translated=$(sysctl -in sysctl.proc_translated 2>/dev/null || true)
[[ "$translated" != 1 ]] || { echo 'translated build host is not an eligible configuration' >&2; exit 1; }
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
sim_library=target/aarch64-apple-ios-sim/release/libminigraf_ffi.a
# macos-15 can be Intel. Keep the requested arm64 simulator slice, and add
# an x86_64 slice when necessary, so execution never depends on translation.
if [[ "$sim_arch" == x86_64 ]]; then
  cargo build --locked --release --lib --target x86_64-apple-ios
  mkdir -p bench/device-farm/build/universal-simulator
  sim_library=bench/device-farm/build/universal-simulator/libminigraf_ffi.a
  xcrun lipo -create target/aarch64-apple-ios-sim/release/libminigraf_ffi.a \
    target/x86_64-apple-ios/release/libminigraf_ffi.a -output "$sim_library"
fi
# Bindgen is a build tool; reuse the host debug dependencies from cargo test.
# Both libraries consumed by XCTest above remain optimized release builds.
cargo run --locked --bin uniffi-bindgen -- generate \
  --library target/aarch64-apple-ios/release/libminigraf_ffi.a \
  --language swift --out-dir bench/device-farm/build/generated
mkdir bench/device-farm/build/includes
cp bench/device-farm/build/generated/*.h bench/device-farm/build/includes/
cp bench/device-farm/build/generated/minigraf_ffiFFI.modulemap bench/device-farm/build/includes/module.modulemap
xcodebuild -create-xcframework \
  -library target/aarch64-apple-ios/release/libminigraf_ffi.a -headers bench/device-farm/build/includes \
  -library "$sim_library" -headers bench/device-farm/build/includes \
  -output bench/device-farm/build/minigrafFFI.xcframework
if [[ "$build_mode" == correctness ]]; then
simulator=$(xcrun simctl list devices available --json | python3 -c '
import json,sys
devices=json.load(sys.stdin)["devices"]
ios=sorted((k for k in devices if "iOS" in k), reverse=True)
print(next(d["udid"] for k in ios for d in devices[k] if d["name"].startswith("iPhone")))')
  selection="$binding_dir/bench/device-farm/build/evidence/selection.json"
  python3 bench/device-farm/qualification/run_simulator.py describe --udid "$simulator" --output "$selection"
else
  simulator=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["udid"])' "$selection")
  cp "$selection" bench/device-farm/build/evidence/selection.json
fi
python3 - "$core_revision" "$binding_revision" "$selection" "$sim_library" "$sim_arch" <<'PY'
import hashlib, json, os, pathlib, subprocess, sys
root = pathlib.Path('bench/device-farm')
digest = hashlib.sha256()
for p in sorted(p for p in root.rglob('*') if p.is_file() and
                not any(part in ('build', 'MinigrafBenchmark.xcodeproj', '__pycache__') for part in p.relative_to(root).parts)):
    digest.update(str(p.relative_to(root)).encode() + b'\0' + p.read_bytes() + b'\0')
def sha(p): return hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()
packages = json.load(open(root/'build/evidence/cargo-metadata.json'))['packages']
uniffi = [p['version'] for p in packages if p['name'] == 'uniffi']
assert len(uniffi) == 1
selection = json.load(open(sys.argv[3]))
device_hash = sha('target/aarch64-apple-ios/release/libminigraf_ffi.a')
simulator_hash = sha(sys.argv[4])
# Verify the packaged archives are the bytes recorded for the two link targets.
archives = list((root/'build/minigrafFFI.xcframework').glob('*/libminigraf_ffi.a'))
assert len(archives) == 2
assert sorted(sha(p) for p in archives) == sorted([device_hash, simulator_hash])
slices = {target: sha('target/'+target+'/release/libminigraf_ffi.a')
          for target in ['aarch64-apple-ios', 'aarch64-apple-ios-sim'] +
          (['x86_64-apple-ios'] if sys.argv[5] == 'x86_64' else [])}
record = dict(core_revision=sys.argv[1], binding_revision=sys.argv[2], harness_revision=sys.argv[2],
    evidence_kind='source-build',
    harness_sha256=digest.hexdigest(), cargo_lock_sha256=sha('Cargo.lock'),
    native_sha256=device_hash,  # Historical #839 device identity, retained for compatibility.
    native_artifacts={'physical': {'sha256': device_hash, 'architecture': 'arm64'},
                      'simulator': {'sha256': simulator_hash, 'architecture': sys.argv[5]}},
    native_slices=slices, host_architecture=sys.argv[5], translation='none',
    runner_image=os.environ.get('ImageOS', 'local'), runner_image_version=os.environ.get('ImageVersion', 'local'),
    swift_binding_sha256=sha(root/'build/generated/minigraf_ffi.swift'),
    rustc=subprocess.check_output(['rustc','--version'],text=True).strip(),
    xcode=subprocess.check_output(['xcodebuild','-version'],text=True).strip(),
    ios_sdk=subprocess.check_output(['xcrun','--sdk','iphoneos','--show-sdk-version'],text=True).strip(),
    ios_sdk_build=subprocess.check_output(['xcrun','--sdk','iphoneos','--show-sdk-build-version'],text=True).strip(),
    simulator_sdk=subprocess.check_output(['xcrun','--sdk','iphonesimulator','--show-sdk-version'],text=True).strip(),
    simulator_sdk_build=subprocess.check_output(['xcrun','--sdk','iphonesimulator','--show-sdk-build-version'],text=True).strip(),
    profile='Release', slowdown_flag='repeat-timed-operation-twice', swift_optimization='-O', rust_profile='release (binding workspace defaults)',
    abi='aarch64-apple-ios', uniffi=uniffi[0], xcodegen='2.46.0')
record.update({key: value for key, value in selection.items() if key != 'udid'})
(root/'build/build.json').write_text(json.dumps(record,indent=2)+'\n')
PY
cp Cargo.lock bench/device-farm/build/evidence/
cd bench/device-farm
curl --fail --location --retry 3 https://github.com/yonaskolb/XcodeGen/releases/download/2.46.0/xcodegen.zip -o build/xcodegen.zip
echo '4d9e34b62172d645eed6457cac13fc222569974098ef4ee9c3368bedf0196806  build/xcodegen.zip' | shasum -a 256 -c -
unzip -q build/xcodegen.zip -d build/tools
build/tools/xcodegen/bin/xcodegen generate
# Explicit allowlist: no timing in the build script, including qualification builds.
xcodebuild test -project MinigrafBenchmark.xcodeproj -scheme MinigrafBenchmark \
  -configuration Release -destination "id=$simulator" -derivedDataPath build/simulator \
  -resultBundlePath build/evidence/correctness.xcresult \
  -only-testing:BenchmarkTests/BenchmarkTests/testOracleRejectsEmptyAndWrongRows \
  -only-testing:BenchmarkTests/BenchmarkTests/testSmoke \
  -only-testing:BenchmarkTests/QualificationWorkTests \
  -parallel-testing-enabled NO \
  CODE_SIGNING_ALLOWED=NO ONLY_ACTIVE_ARCH=YES ARCHS="$sim_arch" | tee build/evidence/correctness.log
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
