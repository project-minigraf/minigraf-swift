import copy
import json
import unittest

from parse_samples import CASES, parse_run


class ParseTests(unittest.TestCase):
    def setUp(self):
        self.identity = dict(core_revision='a'*40, binding_revision='b'*40,
                             harness_revision='b'*40, harness_sha256='c'*64,
                             native_sha256='d'*64)
        self.build = dict(self.identity, profile='Release', slowdown_flag='repeat-timed-operation-twice',
                          native_artifacts={'simulator': {'sha256': 'd'*64, 'architecture': 'arm64'},
                                            'physical': {'sha256': 'e'*64, 'architecture': 'arm64'}})
        self.env = dict(type='environment', mode='qualification', variant='baseline',
                        platform='simulator', build=self.identity, run_id='test-run',
                        model='arm64', os='18.5', profile='Release', double_work=False)
        self.records = [self.env, dict(type='runtime', platform='simulator',
                                      compiled_simulator=True, machine='arm64', os='18.5', architecture='arm64',
                                      simulator_model_identifier='iPhone16,1',
                                      simulator_device_type='com.apple.CoreSimulator.SimDeviceType.iPhone-15',
                                      simulator_runtime='com.apple.CoreSimulator.SimRuntime.iOS-18-5',
                                      simulator_runtime_version='18.5', simulator_runtime_build='test-build')]
        for key in ('simulator_device_type', 'simulator_runtime', 'simulator_runtime_version', 'simulator_runtime_build'):
            self.build[key] = self.records[1][key]
        for key in ('simulator_device_type', 'simulator_runtime'):
            self.env[key] = self.records[1][key]
        for case in CASES:
            full = 'binding/swift-ios/' + case
            iterations = 1 if case in ('file_transact/1k', 'log_writer_append_finish/1k') else 20
            for sample in range(10):
                self.records.append(dict(type='sample', id=full, sample=sample,
                                         iterations=iterations, total_ns=(sample+1)*iterations,
                                         ns_per_op=float(sample+1)))
            self.records.append(dict(type='checked', id=full))
        self.records.append(dict(type='complete', mode='qualification', variant='baseline',
                                 platform='simulator', run_id='test-run', count=14))

    def parse(self, records=None, **kw):
        stream = '\n'.join('2026 XCTest[123] MinigrafBench: ' + json.dumps(r)
                           for r in (records if records is not None else self.records))
        return parse_run(stream, self.build, set_name='B1', run_index=1, attempt=1,
                         platform='simulator', exit_code=kw.pop('exit_code', 0), **kw)

    def test_complete_stream_median_and_lossless_samples(self):
        result = self.parse()
        self.assertTrue(result['valid'])
        for case in CASES:
            entry = result['cases'][case]
            self.assertEqual(entry['value_ns'], 5.5)
            self.assertEqual(entry['sample_means_ns'], list(range(1, 11)))
            self.assertEqual(entry['run_index'], 1)
        self.assertEqual(result['environment'], self.env)
        self.assertEqual(result['records'], self.records)

    def test_slowdown_and_physical_streams(self):
        records = copy.deepcopy(self.records)
        records[0]['variant'] = records[-1]['variant'] = 'slowdown'
        records[0]['double_work'] = True
        stream = '\n'.join(json.dumps(r) for r in records)
        result = parse_run(stream, self.build, set_name='V', run_index=1,
                           attempt=1, platform='simulator', exit_code=0)
        self.assertTrue(result['valid'], result['reason'])
        records[0]['platform'] = records[1]['platform'] = records[-1]['platform'] = 'physical'
        records[0]['model'] = records[1]['machine'] = 'iPhone15,4'
        records[0]['build']['native_sha256'] = 'e'*64
        records[1]['compiled_simulator'] = False
        records[1]['os_build'] = '22F76'
        for record in records[:2]:
            for key in list(record):
                if key.startswith('simulator_'):
                    del record[key]
        stream = '\n'.join(json.dumps(r) for r in records)
        result = parse_run(stream, self.build, set_name='V', run_index=1,
                           attempt=1, platform='physical', exit_code=0)
        self.assertTrue(result['valid'], result['reason'])

    def test_jsonl_also_supported(self):
        stream = '\n'.join(json.dumps(r) for r in self.records)
        self.assertTrue(parse_run(stream, self.build, set_name='B2', run_index=6,
                                 attempt=3, platform='simulator', exit_code=0)['valid'])

    def test_entire_suite_invalid_for_bad_streams(self):
        streams = [self.records[:-1], self.records*2, self.records + [self.records[2]]]
        for field, value in [('mode', 'smoke'), ('variant', 'slowdown'),
                             ('platform', 'physical'), ('profile', 'Debug')]:
            records = copy.deepcopy(self.records)
            records[0][field] = value
            streams.append(records)
        for field, value in [('ns_per_op', 20), ('total_ns', 0), ('sample', 1),
                             ('iterations', 40)]:
            records = copy.deepcopy(self.records)
            records[2][field] = value
            streams.append(records)
        records = copy.deepcopy(self.records)
        records[0]['build']['native_sha256'] = 'e'*64  # device hash on simulator
        streams.append(records)
        records = copy.deepcopy(self.records)
        records[1]['compiled_simulator'] = False
        streams.append(records)
        for records in streams:
            with self.subTest(records=records[:1]):
                result = self.parse(records)
                self.assertFalse(result['valid'])
                self.assertTrue(result['reason'])
                self.assertTrue(all(not c['valid'] and c['value_ns'] is None
                                    for c in result['cases'].values()))

    def test_runtime_and_flag_drift_rejected(self):
        for record_index, field, value in (
                (0, 'double_work', True), (1, 'architecture', 'x86_64'),
                (1, 'simulator_runtime_build', 'other-build'), (1, 'os', '18.6')):
            records = copy.deepcopy(self.records)
            records[record_index][field] = value
            self.assertFalse(self.parse(records)['valid'], field)

    def test_nonfinite_json_returns_serializable_invalid_attempt(self):
        for text in ('NaN', 'Infinity', '1e400'):
            stream = 'MinigrafBench: {"type":"sample","ns_per_op":' + text + '}'
            result = parse_run(stream, self.build, set_name='B1', run_index=1,
                               attempt=1, platform='simulator', exit_code=0)
            self.assertFalse(result['valid'])
            json.dumps(result, allow_nan=False)

    def test_exit_failure_invalidates_completed_stream(self):
        self.assertFalse(self.parse(exit_code=65)['valid'])

    def test_malformed_prefixed_record_is_not_ignored(self):
        result = parse_run('MinigrafBench: {broken', self.build, set_name='B1',
                           run_index=1, attempt=1, platform='simulator', exit_code=0)
        self.assertFalse(result['valid'])


if __name__ == '__main__':
    unittest.main()
