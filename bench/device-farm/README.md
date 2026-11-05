# Swift/iOS release evidence (#839)

This source-build XCTest harness measures fourteen representative binding paths.
It is separate from existing nightly benchmarks. Only physical AWS Device Farm
runs provide #839 timings; GitHub builds and runs correctness checks only.

In disposable binding and core checkouts on Apple Silicon macOS, install Rust
1.99.1 with `aarch64-apple-ios` and `aarch64-apple-ios-sim`, then run:

```sh
bash bench/device-farm/build.sh /path/to/pinned/core
```

The script verifies Cargo resolves that core, runs Rust tests, builds both native
slices and generated Swift, verifies pinned XcodeGen, and runs the oracle and
smoke tests on a simulator in Release configuration. It produces an unsigned
device IPA and XCTest zip under `build/dist` for Device Farm's signing/install
flow. Upload acceptance and a physical smoke are required before measurement;
a successful simulator check alone does not establish device compatibility.

Use Device Farm test type `XCTEST`. Select **only**
`BenchmarkTests/testSmoke` first, then `BenchmarkTests/testBenchmark` for each
timed run. Do not run the whole suite to collect benchmark evidence. Each mode
has its own `Documents/minigraf-bench-MODE.jsonl`; the same records are emitted
with the `MinigrafBench:` NSLog prefix, each under 900 UTF-8 bytes. The compact
build identity must match the full `build.json` shipped beside the IPA.

The common cases cover prepared execution, cursor drains in batches of 1/256,
set round trips, predicates and aggregates. Eight additional cases cover prepare,
first predicate/aggregate registration, typed FactLog batches, rejected options,
read-only opening with options, file transactions and LogWriter append/finish.
Every operation validates exact results, including reopened storage and complete
FactRecords. The negative oracle test rejects empty and wrong same-count/sum rows.

Measurements include binding calls and host JSON decoding/shape checks, matching
the P16 foreign-roundtrip definition. Expected-value comparisons, fixture setup,
registration (except its named cases) and cleanup are outside the timer. Three
warmups precede ten samples averaging 20 operations, or one operation for file
transactions and LogWriter. Swift uses `-O`; Rust uses the binding workspace's
release defaults. This is not the core workspace's size-optimized release profile.
ARC destruction is outside the named timing regions except where the operation
itself closes a cursor/log or finishes a writer.

Retain all samples and per-sample thermal-state observations, device model/OS,
provider run metadata and artifact checksums. Thermal observations are snapshots,
not proof of uninterrupted thermal stability. Public-fleet model/OS selection
does not reserve one physical unit. No RSS bound or release sign-off is implied.
