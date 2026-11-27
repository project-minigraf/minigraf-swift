# Swift/iOS Device Farm harness and #858 qualification

One source harness covers fourteen binding cases in three modes:

| Mode | XCTest methods | Timing use |
| --- | --- | --- |
| Correctness | `BenchmarkTests/testSmoke` and oracle tests | No timing records, either platform |
| #839 benchmark | `BenchmarkTests/testBenchmark` | Physical Device Farm release evidence only |
| #858 qualification | `BenchmarkTests/testQualificationBaseline`, `BenchmarkTests/testQualificationSlowdown` | Matched physical/simulator experiment, artifacts only |

Qualification timing on a hosted iOS simulator is **experimental, not release
evidence and not a nightly lane**. It never goes to Bencher. The explicit
platform identity combines the compile-time simulator target with runtime
`SIMULATOR_MODEL_IDENTIFIER`, `uname` machine and OS evidence. `testBenchmark`
refuses a simulator through that identity check; it has not become a simulator
benchmark. Each environment records `platform`, and qualification has a separate
compact runtime record containing the simulator device type/runtime/version/build
or physical OS build. A simulator's `kern.osversion` is labelled as its **host**
kernel build. Simulator model names are not claims about physical hardware.

## Building

Use disposable, clean binding and core checkouts on macOS. Install Rust 1.99.1
with `aarch64-apple-ios`, `aarch64-apple-ios-sim`, and (on Intel)
`x86_64-apple-ios`. Supply the full lowercase SHA of the checked-out core:

```sh
bash bench/device-farm/build.sh /path/to/pinned/core FULL_40_CHARACTER_CORE_SHA
```

This default remains correctness-only. The script verifies the core checkout
SHA and clean sources, proves Cargo resolves that local core, runs Rust tests,
builds Release native libraries and generated Swift, verifies pinned XcodeGen,
and runs only the smoke and oracle tests on the simulator. It creates the
unsigned device IPA and XCTest zip in `build/dist/` for Device Farm's signing
and installation flow. Upload acceptance and a physical smoke are still needed;
a simulator correctness check cannot establish device compatibility.

`build.json` records core, binding and harness revisions (the harness lives in
the binding commit), a source digest, resolved Cargo lock and generated-binding
hashes, Xcode, both SDK versions/builds, chosen simulator runtime/device type,
runner image/version, architecture and Release flags. `native_slices` contains
the device `aarch64-apple-ios` and simulator `aarch64-apple-ios-sim` hashes.
On Intel, it also contains `x86_64-apple-ios`; the simulator archive is then a
universal archive containing both simulator architectures. `native_artifacts`
records the actual archive linked on each platform, verified against the
XCFramework copies. The runtime selects that platform's hash, never the device
hash for a simulator. On Intel the archive hash describes the universal archive
and the runtime architecture identifies the executed slice. No ISA translation
is requested. The legacy top-level `native_sha256` remains the device hash.

The default correctness workflow remains correctness-only. The manual
`.github/workflows/qualification-858-ios.yml` runs on `macos-15` and accepts
`core-ref` (full SHA) and `schedule` (exactly `B1,V,B2`, repeated six times).
It records `sw_vers`, runner image/version, Xcode and all available runtimes and
device types **before timing**, chooses the exact iPhone 15 device type and
newest available iOS 18.x runtime, and fails if either is absent. It builds once
with the selected configuration:

```sh
python3 bench/device-farm/qualification/run_simulator.py select --output /path/to/selection.json
bash bench/device-farm/build.sh /path/to/core FULL_40_CHARACTER_CORE_SHA qualification /path/to/selection.json
```

The simulator and physical packages share sources and metadata; only their
native targets differ. A changed runner image, runtime, Xcode or architecture
requires a separate configuration and qualification, not pooled observations.
The workflow does not schedule or collect physical Device Farm runs.

## Test selection and fresh processes

Use Device Farm test type `XCTEST`. Select **only**
`BenchmarkTests/testSmoke` first. For #839, select only
`BenchmarkTests/testBenchmark` per timed run. For #858 use
`BenchmarkTests/testQualificationBaseline` for both B1 and B2 and
`BenchmarkTests/testQualificationSlowdown` for V. **Device Farm cannot pass
environment variables to choose this variant**, so it is a distinct test
method. B1/B2 membership belongs to the pre-registered run order, not the code.
Never run the whole XCTest suite as one evidence run.

On the simulator the exact xcodebuild selectors are:

```text
-only-testing:BenchmarkTests/BenchmarkTests/testQualificationBaseline
-only-testing:BenchmarkTests/BenchmarkTests/testQualificationSlowdown
```

`qualification/run_simulator.py run` invokes **a separate
`xcodebuild test-without-building` process per attempt**, selecting exactly one
method and disabling parallel testing. Before **every** attempt (including a
retry), it shuts down the dedicated simulator, erases it, boots it and waits for
`simctl bootstatus -b`. Thus every attempt has a fresh XCTest app process and
fresh Documents directory; no test repetition or Xcode automatic retry is used.
A generated run UUID connects environment/completion records; duplicate UUIDs
are invalid. Device Farm collection must likewise launch a fresh app/process
and retain one complete stream and exit/provider outcome for every attempt.

The fixed order is `(B1,V,B2) × 6`. Only no-valid-stream failures (infrastructure,
validation, process exit or teardown) can retry, at most twice in their original
slot. Every attempt retains `raw.log`, `run.json`, `attempt.json`, and the
`.xcresult` when produced. A valid run is never retried, discarded or replaced
based on latency. Exhaustion is retained as inconclusive; later slots are still
attempted, and the collector exits unsuccessfully. The append-only
`attempts.jsonl` records order, reasons, timestamps and reset/test durations.
An initial invalid receipt is written before launching each attempt, so an
interruption retains a reason even without final parsing. An interrupted job
can also leave an unattempted slot; that too is inconclusive.

The workflow uploads device packages, simulator products, native archives,
`build.json`, logs, parsed runs and checksums, including failures. Artifact
retention is 90 days: archive and verify the checksummed evidence elsewhere
before expiry. Runner queue duration is explicitly unavailable from the runner
environment; retain Actions queue/start timestamps separately. Device Farm
availability/queue time and provider minutes must be recorded by the physical
collector. No cost or timing estimates are presented as observations.

## Timing, slowdown and validation

The common cases cover prepared execution, cursor drains in batches of 1/256,
set round trips, predicates and aggregates. Eight additional cases cover prepare,
first predicate/aggregate registration, FactLog batches, rejected options,
read-only opening, file transactions and LogWriter append/finish.

Measurements include binding calls and host JSON decoding/shape checks,
matching #839/P16. Expected-value comparisons, fixture setup, registration
(except its named cases) and cleanup are outside timing. Three warmups precede
ten sample means averaging 20 logical operations, or one for file transactions
and LogWriter. The run value is the median of those ten means (average of the
middle two). Swift uses `-O`; Rust uses binding workspace release defaults.
ARC destruction stays outside named timing regions except where the operation
closes a cursor/log or finishes a writer. All original baseline/smoke case
operations and validations are retained.

Only `testQualificationSlowdown` sets the harness-private runtime property
`doubleWork`. The timer then encloses **two real executions** per logical
invocation; the denominator remains 20 or 1, never doubled. Two independent
fixtures are prepared outside timing for file transactions, unfinished writers
and first registrations. Read-only opens use two equivalently seeded files,
prepared outside all iterations just as in baseline. Other operations repeat
the same read-only work. Both returned results receive the same complete oracle
outside timing, and both fixtures get cleanup/reopen validation even when a
validation fails. A wrong/empty result or teardown failure invalidates the suite.
The helper and flag exist only in the benchmark XCTest target.

The registered expectation is about +1.0 fractional slowdown (documentation
range 0.7–1.3 for compute/callback and allocation/conversion; storage/startup may
vary). This is not another gate. Use the committed core repository
`docs/benchmarks/858-mobile-qualification/PROTOCOL.md`, `SCHEMA.md` and `decide.py`
for the fixed statistical rules. This harness does not qualify any case or
complete mission #858 without actual matched physical and simulator datasets.

## Streams and schema conversion

Files are `Documents/minigraf-bench-smoke.jsonl`, `...-benchmark.jsonl`,
`...-qualification-baseline.jsonl` and `...-qualification-slowdown.jsonl`.
The same JSON records use the `MinigrafBench:` NSLog prefix, each record under
900 UTF-8 bytes. Qualification environment records carry mode, variant,
platform and the linked native-library identity. Their compact runtime record
contains the remaining runtime pins. Ten `sample` records per case preserve
`total_ns`, original `iterations`, `ns_per_op` and thermal observations.
The qualification completion marker is emitted only after directory teardown.

Convert **one** Documents JSONL or **one** NSLog export, never concatenated copies:

```sh
python3 bench/device-farm/qualification/parse_samples.py raw.log \
  --build build.json --set B1 --run-index 1 --attempt 1 \
  --platform physical --exit-code 0 --output run.json
```

Supply the actual test/provider exit outcome (`0` only for success). The parser
checks complete ordered cases, all ten samples, exact-result markers, completion,
mode/variant/platform, revisions, native archive hash, Release configuration,
logical denominators and the sample arithmetic. It rejects duplicated, truncated
or mixed streams and nonzero exits for the **entire suite**. It never filters a
case by its value. `run.json` retains environment/runtime and the original
records; `cases[CASE]` is the SCHEMA.md per-run entry, including all ten
`sample_means_ns` and their unrounded `value_ns`. Append that entry to
`configurations[…].cases[CASE].sets[run.set]`, retaining invalid attempts too.
SCHEMA suffixes omit the legacy harness prefix `binding/swift-ios/`. Map runtime
`platform=simulator` to schema `platform=ios, kind=simulated` (physical to
`kind=physical`). For each set, use `environment.build.native_sha256` as its
configuration artifact hash. Verify identical runtime/configuration pins across
all attempts and the matched physical/simulated source revisions before assembly;
retain original streams and manifests alongside `raw-samples.json`.

Local standard-library checks:

```sh
python3 -m unittest discover -s bench/device-farm/qualification -p 'test_*.py'
python3 -m py_compile bench/device-farm/qualification/*.py
bash -n bench/device-farm/build.sh
```

`QualificationWorkTests` are pure XCTest controls for independent fixtures,
timing boundaries, wrong/empty results from either operation, setup/action
failures and cleanup. They need a Swift/XCTest toolchain but no device or native
library. The build's correctness allowlist runs them alongside the existing
`BenchmarkTests/testOracleRejectsEmptyAndWrongRows` and smoke test. On Linux
without Swift/Xcode, only the Python/shell controls can be run; a macOS build and
physical acceptance are still required.

Thermal observations are snapshots, not proof of uninterrupted stability. Public
fleet selection does not reserve a physical unit. No RSS bound or release
sign-off is implied.
