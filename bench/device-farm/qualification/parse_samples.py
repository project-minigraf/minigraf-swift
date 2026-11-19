#!/usr/bin/env python3
"""Validate exactly one qualification stream; retain lossless records and schema entries.

Use either Documents JSONL OR one NSLog export, never their concatenation. No
latency-based filtering. A failed suite invalidates all fourteen case entries.
Exit 1 means no valid stream (eligible for the protocol's bounded retry).
"""
import argparse
import json
import math
from pathlib import Path
import re
import statistics

CASES = (
    'prepared_execute/1k', 'cursor_batch_1/1k', 'cursor_batch_256/1k',
    'set_roundtrip/2', 'predicate/1k', 'aggregate/1k', 'prepare/point',
    'register_predicate/first', 'register_aggregate/first', 'fact_log_batch_256/1k',
    'options_error/negative_cache', 'options_readonly_open/1k', 'file_transact/1k',
    'log_writer_append_finish/1k',
)
PREFIX = 'binding/swift-ios/'
IDENTITY = ('core_revision', 'binding_revision', 'harness_revision', 'harness_sha256')


def require(condition, message):
    if not condition:
        raise ValueError(message)


def positive(value):
    return type(value) in (int, float) and math.isfinite(value) and value > 0


def finite_float(text):
    value = float(text)
    require(math.isfinite(value), 'nonfinite JSON number')
    return value


def invalid_constant(text):
    raise ValueError('nonfinite JSON constant: ' + text)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, 'duplicate JSON key: ' + key)
        result[key] = value
    return result


def parse_run(stream, build, *, set_name, run_index, attempt, platform, exit_code):
    require(set_name in ('B1', 'B2', 'V'), 'unknown set')
    require(type(run_index) is int and 1 <= run_index <= 6, 'run index outside 1..6')
    require(type(attempt) is int and 1 <= attempt <= 3, 'attempt outside 1..3')
    require(platform in ('physical', 'simulator'), 'unknown platform')
    variant = 'slowdown' if set_name == 'V' else 'baseline'
    records, environment, runtime, reason = [], None, None, ''
    samples = {case: [] for case in CASES}
    try:
        for line in stream.splitlines():
            if 'MinigrafBench:' in line:
                line = line.split('MinigrafBench:', 1)[1].strip()
            elif not line.lstrip().startswith('{'):
                continue
            record = json.loads(line, object_pairs_hook=unique_object,
                                parse_float=finite_float, parse_constant=invalid_constant)
            require(isinstance(record, dict), 'nonobject record')
            records.append(record)
        require(exit_code == 0, f'process/teardown failed (exit {exit_code})')
        require(len(records) == 157, 'missing, extra or duplicate records')
        environment, runtime = records[:2]
        require(environment.get('type') == 'environment', 'missing initial environment')
        require(runtime.get('type') == 'runtime', 'missing runtime identity')
        require(build['profile'] == environment.get('profile') == 'Release', 'not Release')
        require(build.get('slowdown_flag') == 'repeat-timed-operation-twice', 'wrong source flag')
        require(environment.get('double_work') is (set_name == 'V'), 'wrong slowdown flag state')
        for record in (environment, records[-1]):
            require(record.get('mode') == 'qualification', 'mixed/nonqualification mode')
            require(record.get('variant') == variant, 'wrong variant for set')
            require(record.get('platform') == platform, 'wrong platform')
        identity = environment['build']
        for key in IDENTITY:
            length = 64 if key.endswith('sha256') else 40
            require(re.fullmatch('[0-9a-f]{%d}' % length, identity[key]) is not None,
                    'invalid ' + key)
            require(identity[key] == build[key], 'mismatched ' + key)
        native = build['native_artifacts'][platform]
        require(runtime.get('architecture') == native['architecture'], 'wrong executed architecture')
        artifact = native['sha256']
        require(re.fullmatch('[0-9a-f]{64}', artifact) is not None, 'invalid native hash')
        require(identity['native_sha256'] == artifact, 'wrong executed native artifact')
        require(runtime.get('platform') == platform, 'runtime platform mismatch')
        require(runtime.get('compiled_simulator') is (platform == 'simulator'),
                'compile-time/runtime platform conflict')
        require(runtime.get('machine') and runtime.get('os'), 'missing runtime evidence')
        require(environment.get('model') == runtime['machine'] and environment.get('os') == runtime['os'],
                'conflicting environment/runtime evidence')
        if platform == 'simulator':
            require(runtime.get('simulator_model_identifier'), 'missing simulator model evidence')
            for field in ('simulator_device_type', 'simulator_runtime',
                          'simulator_runtime_version', 'simulator_runtime_build'):
                require(runtime.get(field), 'missing ' + field)
                require(runtime[field] == build[field], 'mismatched ' + field)
            for field in ('simulator_device_type', 'simulator_runtime'):
                require(environment.get(field) == runtime[field], 'conflicting ' + field)
            require(runtime['simulator_runtime_version'] in (runtime['os'], runtime['os'] + '.0'),
                    'simulator OS differs from pinned runtime')
        else:
            require(runtime.get('os_build'), 'missing physical OS build')
            require(not runtime.get('simulator_model_identifier'), 'simulator on physical path')
            require(runtime['machine'].startswith(('iPhone', 'iPad', 'iPod')), 'not physical iOS')
        require(environment.get('run_id'), 'missing process run identity')
        complete = records[-1]
        require(complete.get('type') == 'complete' and complete.get('count') == 14,
                'missing completion after teardown')
        require(complete.get('run_id') == environment['run_id'], 'mixed run identities')
        offset = 2
        for case in CASES:
            full_id = PREFIX + case
            iterations = 1 if case in ('file_transact/1k', 'log_writer_append_finish/1k') else 20
            for index in range(10):
                sample = records[offset]
                offset += 1
                require(sample.get('type') == 'sample' and sample.get('id') == full_id,
                        'wrong case/sample order')
                require(type(sample.get('sample')) is int and sample['sample'] == index,
                        'missing/duplicate sample')
                require(type(sample.get('iterations')) is int and sample['iterations'] == iterations,
                        'changed logical denominator')
                require(positive(sample.get('total_ns')) and positive(sample.get('ns_per_op')),
                        'nonpositive/nonfinite timing')
                mean = sample['total_ns'] / iterations
                require(mean == sample['ns_per_op'], 'mean differs from total / iterations')
                samples[case].append(mean)
            checked = records[offset]
            offset += 1
            require(checked.get('type') == 'checked' and checked.get('id') == full_id,
                    'missing exact-result check')
    except (ValueError, KeyError, TypeError, IndexError, OverflowError) as error:
        reason = 'no valid stream: ' + str(error)
    valid = not reason
    cases = {}
    for case in CASES:
        entry = dict(run_index=run_index, attempt=attempt, valid=valid, reason=reason,
                     value_ns=statistics.median(samples[case]) if valid else None)
        if samples[case]:
            entry['sample_means_ns'] = samples[case]
        cases[case] = entry
    return dict(schema_version=1, set=set_name, valid=valid, reason=reason,
                environment=environment, runtime=runtime, cases=cases, records=records)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('stream', type=Path)
    parser.add_argument('--build', required=True, type=Path)
    parser.add_argument('--set', dest='set_name', required=True, choices=('B1', 'B2', 'V'))
    parser.add_argument('--run-index', required=True, type=int)
    parser.add_argument('--attempt', required=True, type=int)
    parser.add_argument('--platform', required=True, choices=('physical', 'simulator'))
    parser.add_argument('--exit-code', required=True, type=int,
                        help='actual XCTest/provider exit status; a stream alone is insufficient')
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    result = parse_run(args.stream.read_text(), json.loads(args.build.read_text()),
                       set_name=args.set_name, run_index=args.run_index, attempt=args.attempt,
                       platform=args.platform, exit_code=args.exit_code)
    args.output.write_text(json.dumps(result, indent=2, sort_keys=True, allow_nan=False) + '\n')
    return 0 if result['valid'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
