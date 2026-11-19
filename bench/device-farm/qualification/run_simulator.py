#!/usr/bin/env python3
"""#858 simulator selection and fresh-process collection; artifacts only."""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import subprocess
import time

from parse_samples import parse_run

DEVICE_TYPE = 'com.apple.CoreSimulator.SimDeviceType.iPhone-15'


def slots(schedule):
    if schedule != 'B1,V,B2':
        raise ValueError('pre-registered schedule is B1,V,B2, repeated six times')
    return [(index, name) for index in range(1, 7) for name in ('B1', 'V', 'B2')]


def collect(schedule, execute):
    complete = True
    for index, name in slots(schedule):
        for attempt in range(1, 4):
            if execute(index, name, attempt):
                break
        else:
            complete = False
    return complete


def select_simulator(runtimes, types):
    if not any(t['identifier'] == DEVICE_TYPE for t in types['devicetypes']):
        raise ValueError('pinned iPhone 15 device type is unavailable; no substitution')
    available = [r for r in runtimes['runtimes'] if r.get('isAvailable') and
                 r['identifier'].startswith('com.apple.CoreSimulator.SimRuntime.iOS-') and
                 r['version'].split('.')[0] == '18']
    if not available:
        raise ValueError('no available iOS 18.x runtime; no substitution')
    runtime = max(available, key=lambda r: tuple(int(n) for n in r['version'].split('.')))
    return dict(simulator_device_type=DEVICE_TYPE, simulator_runtime=runtime['identifier'],
                simulator_runtime_version=runtime['version'], simulator_runtime_build=runtime['buildversion'])


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True, allow_nan=False) + '\n')


def command_json(*args):
    return json.loads(subprocess.check_output(args, text=True, stdin=subprocess.DEVNULL))


def choose(output):
    runtimes = command_json('xcrun', 'simctl', 'list', 'runtimes', '--json')
    types = command_json('xcrun', 'simctl', 'list', 'devicetypes', '--json')
    choice = select_simulator(runtimes, types)
    choice['udid'] = subprocess.check_output(
        ['xcrun', 'simctl', 'create', 'Minigraf-858', choice['simulator_device_type'],
         choice['simulator_runtime']], text=True, stdin=subprocess.DEVNULL).strip()
    write_json(output, choice)


def describe(udid, output):
    """Record the existing simulator selected by the correctness-only build."""
    devices = command_json('xcrun', 'simctl', 'list', 'devices', 'available', '--json')['devices']
    runtimes = command_json('xcrun', 'simctl', 'list', 'runtimes', '--json')['runtimes']
    runtime_id, device = next((r, d) for r, ds in devices.items() for d in ds if d['udid'] == udid)
    runtime = next(r for r in runtimes if r['identifier'] == runtime_id)
    write_json(output, dict(udid=udid, simulator_device_type=device['deviceTypeIdentifier'],
                           simulator_runtime=runtime_id, simulator_runtime_version=runtime['version'],
                           simulator_runtime_build=runtime['buildversion']))


def run(args):
    slots(args.schedule)  # Validate before touching the simulator or measuring.
    selection = json.loads(args.selection.read_text())
    build = json.loads(args.build.read_text())
    for key, value in selection.items():
        if key != 'udid' and build.get(key) != value:
            raise ValueError('selected simulator differs from build manifest: ' + key)
    args.output.mkdir(parents=True, exist_ok=False)  # Never accept stale output.
    udid = selection['udid']
    seen_run_ids = set()
    pinned_runtime = None

    def execute(index, name, attempt):
        nonlocal pinned_runtime
        destination = args.output / f'{index:02d}-{name}-attempt-{attempt}'
        destination.mkdir()
        started = datetime.now(timezone.utc).isoformat()
        start = time.monotonic()
        method = 'testQualificationSlowdown' if name == 'V' else 'testQualificationBaseline'
        command = ['xcodebuild', 'test-without-building', '-project', 'MinigrafBenchmark.xcodeproj',
                   '-scheme', 'MinigrafBenchmark', '-configuration', 'Release',
                   '-destination', f'id={udid}', '-derivedDataPath', 'build/simulator',
                   '-resultBundlePath', str(destination / 'result.xcresult'),
                   '-parallel-testing-enabled', 'NO', '-maximum-concurrent-test-simulator-destinations', '1',
                   '-only-testing:BenchmarkTests/BenchmarkTests/' + method,
                   'CODE_SIGNING_ALLOWED=NO']
        # Write an invalid receipt before starting external work. If the job is
        # cancelled, partial logs still have a slot, attempt and explicit reason.
        pending_reason = 'no valid stream: attempt started; final result not recorded (interrupted if retained)'
        pending = parse_run('', build, set_name=name, run_index=index, attempt=attempt,
                            platform='simulator', exit_code=1)
        pending['reason'] = pending_reason
        for entry in pending['cases'].values():
            entry['reason'] = pending_reason
        write_json(destination / 'run.json', pending)
        write_json(destination / 'attempt.json', dict(run_index=index, set=name, attempt=attempt,
                   valid=False, reason=pending_reason, started_utc=started, command=command))
        code, reason = 1, ''
        reset_start = time.monotonic()
        measurement_start = None
        with (destination / 'raw.log').open('w') as log:
            try:
                # A failed prior process is killed by shutdown. Erase all app data before
                # every attempt, including retries, then boot and wait for readiness.
                devices = command_json('xcrun', 'simctl', 'list', 'devices', '--json')['devices']
                runtime_id, device = next((r, d) for r, ds in devices.items() for d in ds if d['udid'] == udid)
                if (runtime_id != selection['simulator_runtime'] or
                        device['deviceTypeIdentifier'] != selection['simulator_device_type']):
                    raise ValueError('executing simulator device type/runtime differs from selection')
                runtimes = command_json('xcrun', 'simctl', 'list', 'runtimes', '--json')['runtimes']
                runtime = next(r for r in runtimes if r['identifier'] == runtime_id)
                if (runtime['version'] != selection['simulator_runtime_version'] or
                        runtime['buildversion'] != selection['simulator_runtime_build']):
                    raise ValueError('executing simulator runtime version/build changed')
                if device['state'] != 'Shutdown':
                    subprocess.run(['xcrun', 'simctl', 'shutdown', udid], check=True,
                                   stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT, timeout=120)
                for operation in (['erase', udid], ['boot', udid], ['bootstatus', udid, '-b']):
                    subprocess.run(['xcrun', 'simctl', *operation], check=True,
                                   stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT, timeout=180)
                measurement_start = time.monotonic()
                # Each call launches a new XCTest host process. No -retry-tests-on-failure,
                # test repetitions, shared runner, or measurement-dependent reruns.
                code = subprocess.run(command, cwd=args.harness, stdin=subprocess.DEVNULL,
                                      stdout=log, stderr=subprocess.STDOUT, timeout=1200).returncode
            except (OSError, subprocess.SubprocessError, StopIteration, ValueError, KeyError) as error:
                reason = f'infrastructure failure: {type(error).__name__}: {error}'
                log.write('\n' + reason + '\n')
        result = parse_run((destination / 'raw.log').read_text(errors='replace'), build,
                           set_name=name, run_index=index, attempt=attempt,
                           platform='simulator', exit_code=code)
        if result['valid']:
            run_id = result['environment']['run_id']
            if run_id in seen_run_ids:
                reason = 'no valid stream: repeated process run identity'
            else:
                seen_run_ids.add(run_id)
            if pinned_runtime is None:
                pinned_runtime = result['runtime']
            elif result['runtime'] != pinned_runtime:
                reason = 'no valid stream: runtime configuration changed within collection'
        if reason:
            result['valid'], result['reason'] = False, reason
            for entry in result['cases'].values():
                entry.update(valid=False, reason=reason, value_ns=None)
        write_json(destination / 'run.json', result)
        now = time.monotonic()
        manifest = dict(run_index=index, set=name, attempt=attempt, valid=result['valid'],
                        reason=result['reason'], started_utc=started, elapsed_seconds=now-start,
                        reset_seconds=(measurement_start or now)-reset_start,
                        test_seconds=now-measurement_start if measurement_start else None,
                        exit_code=code, test=method, process_policy='shutdown-erase-boot-per-attempt',
                        simulator=selection, command=command,
                        runner_image=os.environ.get('ImageVersion', 'unavailable'))
        write_json(destination / 'attempt.json', manifest)
        with (args.output / 'attempts.jsonl').open('a') as output:
            output.write(json.dumps(manifest, sort_keys=True) + '\n')
        print(f'{index} {name} attempt {attempt}: {"valid" if result["valid"] else result["reason"]}', flush=True)
        return result['valid']

    return 0 if collect(args.schedule, execute) else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='operation', required=True)
    select = commands.add_parser('select')
    select.add_argument('--output', type=Path, required=True)
    description = commands.add_parser('describe')
    description.add_argument('--udid', required=True)
    description.add_argument('--output', type=Path, required=True)
    collect_parser = commands.add_parser('run')
    for field in ('selection', 'build', 'output', 'harness'):
        collect_parser.add_argument('--' + field, type=lambda p: Path(p).resolve(), required=True)
    collect_parser.add_argument('--schedule', required=True)
    args = parser.parse_args()
    if args.operation == 'select':
        choose(args.output)
    elif args.operation == 'describe':
        describe(args.udid, args.output)
    else:
        return run(args)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
