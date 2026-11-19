import contextlib
import copy
import io
import json
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import patch

from run_simulator import collect, run, select_simulator, slots


class DriverTests(unittest.TestCase):
    def test_only_registered_order(self):
        self.assertEqual(slots('B1,V,B2'), [(i, s) for i in range(1, 7) for s in ('B1', 'V', 'B2')])
        for value in ('B1,B2,V', 'B1', ''):
            with self.assertRaises(ValueError):
                slots(value)

    def test_retry_stays_in_slot_and_never_replaces_valid(self):
        calls = []
        def attempt(index, name, number):
            calls.append((index, name, number))
            return not (index == 1 and name == 'B1' and number < 3)
        self.assertTrue(collect('B1,V,B2', attempt))
        self.assertEqual(calls[:4], [(1, 'B1', 1), (1, 'B1', 2), (1, 'B1', 3), (1, 'V', 1)])
        self.assertEqual(len(calls), 20)

    def test_exhaustion_retains_three_attempts_then_continues(self):
        calls = []
        def attempt(index, name, number):
            calls.append((index, name, number))
            return name != 'V'
        self.assertFalse(collect('B1,V,B2', attempt))
        self.assertEqual(len(calls), 30)
        self.assertEqual(calls[-1], (6, 'B2', 1))

    def test_driver_retains_failed_attempt_and_uses_fresh_commands(self):
        from test_parse_samples import ParseTests
        fixture = ParseTests()
        fixture.setUp()
        selection = {key: value for key, value in fixture.build.items() if key.startswith('simulator_')}
        selection['udid'] = 'synthetic-simulator'
        commands = []
        processes = []

        def inventory(*command):
            if 'devices' in command:
                return {'devices': {selection['simulator_runtime']: [
                    dict(udid=selection['udid'], state='Booted',
                         deviceTypeIdentifier=selection['simulator_device_type'])]}}
            return {'runtimes': [dict(identifier=selection['simulator_runtime'], version='18.5',
                                      buildversion='test-build')]}

        def execute(command, **kwargs):
            commands.append(command)
            if command[0] != 'xcodebuild':
                return SimpleNamespace(returncode=0)
            processes.append(command)
            attempt_dir = Path(kwargs['stdout'].name).parent
            pending = json.loads((attempt_dir/'run.json').read_text())
            self.assertFalse(pending['valid'])
            self.assertIn('interrupted', pending['reason'])
            self.assertTrue((attempt_dir/'attempt.json').is_file())
            records = copy.deepcopy(fixture.records)
            records[0]['run_id'] = records[-1]['run_id'] = f'synthetic-run-{len(processes)}'
            slowdown = command[-2].endswith('testQualificationSlowdown')
            records[0]['variant'] = records[-1]['variant'] = 'slowdown' if slowdown else 'baseline'
            records[0]['double_work'] = slowdown
            kwargs['stdout'].write('\n'.join('MinigrafBench: ' + json.dumps(r) for r in records))
            return SimpleNamespace(returncode=65 if len(processes) == 1 else 0)

        # All test scratch stays inside this extracted snapshot.
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as directory:
            root = Path(directory).resolve()
            (root/'selection.json').write_text(json.dumps(selection))
            (root/'build.json').write_text(json.dumps(fixture.build))
            args = SimpleNamespace(schedule='B1,V,B2', selection=root/'selection.json',
                                   build=root/'build.json', output=root/'runs', harness=root)
            with patch('run_simulator.command_json', side_effect=inventory), \
                    patch('run_simulator.subprocess.run', side_effect=execute), \
                    contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(run(args), 0)
            attempts = [json.loads(line) for line in (root/'runs/attempts.jsonl').read_text().splitlines()]
            self.assertEqual(len(attempts), 19)
            self.assertFalse(attempts[0]['valid'])
            self.assertIn('exit 65', attempts[0]['reason'])
            self.assertEqual([a['attempt'] for a in attempts[:3]], [1, 2, 1])
            self.assertTrue(all(a['valid'] for a in attempts[1:]))
            self.assertEqual(len(list((root/'runs').glob('*/raw.log'))), 19)
            self.assertEqual(len(list((root/'runs').glob('*/run.json'))), 19)
            for i, command in enumerate(processes):
                self.assertEqual(command[1], 'test-without-building')
                self.assertEqual(sum(c.startswith('-only-testing:') for c in command), 1)
                self.assertEqual(commands[i*5:i*5+4], [
                    ['xcrun', 'simctl', 'shutdown', selection['udid']],
                    ['xcrun', 'simctl', 'erase', selection['udid']],
                    ['xcrun', 'simctl', 'boot', selection['udid']],
                    ['xcrun', 'simctl', 'bootstatus', selection['udid'], '-b']])

    def test_newest_available_ios18_and_exact_iphone15(self):
        runtimes = {'runtimes': [
            dict(identifier='old', version='18.4', buildversion='a', isAvailable=True),
            dict(identifier='new', version='18.5', buildversion='b', isAvailable=True),
            dict(identifier='future', version='19.0', buildversion='c', isAvailable=True),
            dict(identifier='missing', version='18.6', buildversion='d', isAvailable=False),
        ]}
        for r in runtimes['runtimes']:
            r['identifier'] = 'com.apple.CoreSimulator.SimRuntime.iOS-' + r['identifier']
        types = {'devicetypes': [dict(name='iPhone 15', identifier='com.apple.CoreSimulator.SimDeviceType.iPhone-15')]}
        selection = select_simulator(runtimes, types)
        self.assertEqual(selection['simulator_runtime_version'], '18.5')
        self.assertEqual(selection['simulator_runtime_build'], 'b')
        with self.assertRaises(ValueError):
            select_simulator(runtimes, {'devicetypes': []})
        with self.assertRaises(ValueError):
            select_simulator({'runtimes': []}, types)


if __name__ == '__main__':
    unittest.main()
