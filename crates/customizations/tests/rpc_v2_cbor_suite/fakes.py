"""Writes fakes.json: what botocore's RpcV2CBORSerializer and RpcV2CBORParser
give over the zcbor and zcborqc fake models (tests/models), for the requests
and replies listed here. Run from the repository root with a Python that has
botocore's dependencies (dateutil):

    python crates/customizations/tests/rpc_v2_cbor_suite/fakes.py > \
        crates/customizations/tests/rpc_v2_cbor_suite/fakes.json

JSON spellings: {"$blob": base64} is bytes, {"$float": repr} a float JSON
cannot hold, {"$int": text} an int past 64 bits.
"""
import base64
import json
import math
import os
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '../../../..'))
sys.path.insert(0, os.path.join(ROOT, 'aws-cli', 'awscli'))

from botocore.awsrequest import HeadersDict  # noqa: E402
from botocore.model import OperationModel, ServiceModel  # noqa: E402
from botocore.parsers import RpcV2CBORParser  # noqa: E402
from botocore.serialize import RpcV2CBORSerializer  # noqa: E402


def model(name):
    path = os.path.join(ROOT, 'tests', 'models', name, '2020-01-01', 'service-2.json')
    with open(path) as f:
        return ServiceModel(json.load(f), service_name=name)


def decode(value):
    if isinstance(value, dict):
        if set(value) == {'$blob'}:
            return base64.b64decode(value['$blob'])
        if set(value) == {'$float'}:
            return float(value['$float'])
        if set(value) == {'$int'}:
            return int(value['$int'])
        return {k: decode(v) for k, v in value.items()}
    if isinstance(value, list):
        return [decode(v) for v in value]
    return value


def encode(value):
    if isinstance(value, bytes):
        return {'$blob': base64.b64encode(value).decode()}
    if isinstance(value, float) and not math.isfinite(value):
        return {'$float': repr(value)}
    if isinstance(value, int) and not isinstance(value, bool) and not -2**63 <= value < 2**63:
        return {'$int': str(value)}
    if isinstance(value, dict):
        return {k: encode(v) for k, v in value.items()}
    if isinstance(value, list):
        return [encode(v) for v in value]
    return value


BLOB = {'$blob': 'AAFi'}
NAN, INF = {'$float': 'nan'}, {'$float': 'inf'}
REQUESTS = [
    ('fakes-zcbor-put-record', 'zcbor', 'PutRecord', {
        'Flag': True, 'Ratio': 1.5, 'Count': 3, 'When': '2015-08-30T12:36:00Z',
        'Items': ['a', 'b'], 'Grid': [['a', 'b'], ['c']], 'Groups': {'g': ['x', 'y']},
        'Labels': {'k': 'v'}, 'Parts': [{'Name': 'p', 'Size': 1}],
        'Index': {'i': {'Name': 'q', 'Size': 2}}, 'Part': {'Name': 'r', 'Size': 3},
        'Choice': {'Word': 'w'}, 'Note': 'n', 'Name': 'n1', 'Token': 'tok-1', 'Data': BLOB}),
    ('fakes-zcbor-put-record-token-filled', 'zcbor', 'PutRecord', {'Note': 'n'}),
    ('fakes-zcbor-get-record', 'zcbor', 'GetRecord', {'Id': 'r-1'}),
    ('empty', 'zcbor', 'PutRecord', {}),
    ('none-member-dropped', 'zcbor', 'PutRecord', {'Note': None, 'Count': 1}),
    ('none-items-kept', 'zcbor', 'PutRecord', {'Items': ['a', None], 'Labels': {'k': None}}),
    ('ints', 'zcbor', 'PutRecord', {'Count': -1, 'Parts': [{'Size': 23}, {'Size': 24},
        {'Size': 255}, {'Size': 256}, {'Size': -25}, {'Size': 65536}, {'Size': -2147483648}]}),
    ('long-text', 'zcbor', 'PutRecord', {'Note': 'é' * 30 + 'x' * 300}),
    ('many-items', 'zcbor', 'PutRecord', {'Items': ['x'] * 300}),
    ('timestamp-fraction', 'zcbor', 'PutRecord', {'When': '2015-08-30T12:36:00.5Z'}),
    ('timestamp-number', 'zcbor', 'PutRecord', {'When': 1440938160}),
    ('timestamp-micros', 'zcbor', 'PutRecord', {'When': '1970-01-01T00:00:01.003691Z'}),
    ('blob-from-text', 'zcbor', 'PutRecord', {'Data': 'text é'}),
    ('doubles', 'zcbor', 'PutRecord', {'Ratio': 5e-324}),
    ('double-nan', 'zcbor', 'PutRecord', {'Ratio': NAN}),
    ('double-inf', 'zcbor', 'PutRecord', {'Ratio': INF}),
    ('double-neg-inf', 'zcbor', 'PutRecord', {'Ratio': {'$float': '-inf'}}),
    ('double-from-int', 'zcbor', 'PutRecord', {'Ratio': 3}),
    ('union-number', 'zcbor', 'PutRecord', {'Choice': {'Number': 7}}),
    ('fakes-zcborqc-send-metric', 'zcborqc', 'SendMetric', {
        'Name': 'm', 'Value': 0.1, 'Values': [1.5, 2.0], 'Count': 24, 'Total': 2**40,
        'Stamp': '2015-08-30T12:36:00Z', 'Payload': BLOB, 'Enabled': False}),
    ('float-specials', 'zcborqc', 'SendMetric', {'Name': 'm', 'Value': INF}),
    ('float-nan', 'zcborqc', 'SendMetric', {'Name': 'm', 'Value': NAN}),
    ('float-tiny', 'zcborqc', 'SendMetric', {'Name': 'm', 'Value': 1e-50}),
    ('float-too-large', 'zcborqc', 'SendMetric', {'Name': 'm', 'Value': 1e39}),
    ('long-min', 'zcborqc', 'SendMetric', {'Name': 'm', 'Total': {'$int': str(-2**64)}}),
    ('long-max', 'zcborqc', 'SendMetric', {'Name': 'm', 'Total': {'$int': str(2**64 - 1)}}),
    ('long-too-big', 'zcborqc', 'SendMetric', {'Name': 'm', 'Total': {'$int': str(2**64)}}),
    ('fakes-zcborqc-ping', 'zcborqc', 'Ping', {}),
]


def text(value):
    data = value.encode()
    return head(3, len(data)) + data


def head(major, n):
    if n < 24:
        return bytes([major << 5 | n])
    for info, size in ((24, 1), (25, 2), (26, 4), (27, 8)):
        if n < 256 ** size:
            return bytes([major << 5 | info]) + n.to_bytes(size, 'big')


def cmap(*pairs):
    out = head(5, len(pairs))
    for key, value in pairs:
        out += (text(key) if isinstance(key, str) else key) + value
    return out


def h(hexes):
    return bytes.fromhex(hexes.replace(' ', ''))


CBOR = [('Content-Type', 'application/cbor'), ('smithy-protocol', 'rpc-v2-cbor'),
        ('x-amzn-RequestId', 'fake-c')]
GET = 'r2RGbGFn9WVSYXRpb/s/+AAAAAAAAGVDb3VudANlVG90YWwbACAAAAAAAAFkV2hlbsH7QdV4viwgAABlSXRlbXOCYWFhYmRHcmlkgoFhYYJhYmFjZkdyb3Vwc6FhZ4FheGZMYWJlbHOhYWthdmVQYXJ0c4GiZE5hbWVhcGRTaXplAWVJbmRleKFhaaJkTmFtZWFxZFNpemUCZFBhcnSiZE5hbWVhcmRTaXplA2ZDaG9pY2WhZk51bWJlcgdkTm90ZWFuZE1hcHOCoWFhYWKg'
NOT_FOUND = 'pWZfX3R5cGV4IGNvbS5leGFtcGxlLnpjYm9yI1JlY29yZE5vdEZvdW5kZ01lc3NhZ2Vpbm8gcmVjb3JkaEF0dGVtcHRzAmdSZWFzb25zgWRnb25lZVBhcnRzgaJkTmFtZWFwZFNpemUB'
QC = [('x-amzn-query-error', 'AWS.Zcborqc.QueueMissing;Sender')] + CBOR
RESPONSES = [
    ('fakes-zcbor-get-record', 'zcbor', 'GetRecord', 200, CBOR, base64.b64decode(GET)),
    ('fakes-zcbor-get-record-not-found', 'zcbor', 'GetRecord', 404, CBOR,
     base64.b64decode(NOT_FOUND)),
    ('unmodeled-error', 'zcbor', 'GetRecord', 400, CBOR,
     cmap(('__type', text('aws.protocoltests#Throttled:http://x')), ('message', text('slow')))),
    ('message-none', 'zcbor', 'GetRecord', 400, CBOR, cmap(('message', h('f6')))),
    ('error-status-code', 'zcbor', 'GetRecord', 418, [], cmap()),
    ('server-error-empty', 'zcbor', 'GetRecord', 503, CBOR, b''),
    ('client-error-empty', 'zcbor', 'GetRecord', 400, CBOR, b''),
    ('error-not-a-map', 'zcbor', 'GetRecord', 400, CBOR, h('8101')),
    ('error-int-code', 'zcbor', 'GetRecord', 400, CBOR, cmap(('__type', h('07')))),
    ('output-empty', 'zcbor', 'GetRecord', 200, CBOR, b''),
    ('output-not-a-map', 'zcbor', 'GetRecord', 200, CBOR, h('07')),
    ('output-nulls-and-extras', 'zcbor', 'GetRecord', 200, CBOR,
     cmap(('Note', h('f6')), ('Extra', h('01')), ('Count', h('01')), ('Note', h('f7')))),
    ('output-duplicates', 'zcbor', 'GetRecord', 200, CBOR,
     cmap(('Labels', cmap(('a', text('1')), ('b', text('2')), ('a', text('3')))),
          ('Count', h('05')), ('Count', h('06')))),
    ('output-raw-maps', 'zcbor', 'GetRecord', 200, CBOR,
     cmap(('Index', cmap(('i', cmap(('Size', h('c101')), ('Other', h('f6')), ('Name', h('41ff')))))),
          ('Groups', cmap(('g', h('80')), ('h', h('f6')))))),
    ('output-list-of-text', 'zcbor', 'GetRecord', 200, CBOR,
     cmap(('Items', text('ab')), ('Grid', h('82 f6 81 f6')))),
    ('output-list-of-bytes', 'zcbor', 'GetRecord', 200, CBOR, cmap(('Items', h('420102')))),
    ('output-list-of-map', 'zcbor', 'GetRecord', 200, CBOR, cmap(('Items', cmap(('k', h('01')))))),
    ('output-list-not-iterable', 'zcbor', 'GetRecord', 200, CBOR, cmap(('Items', h('01')))),
    ('output-union-two', 'zcbor', 'GetRecord', 200, CBOR,
     cmap(('Choice', cmap(('Word', text('w')), ('Number', h('01')))))),
    ('output-union-unknown', 'zcbor', 'GetRecord', 200, CBOR,
     cmap(('Choice', cmap(('Other', text('w')), ('__type', text('t')))))),
    ('output-union-none', 'zcbor', 'GetRecord', 200, CBOR,
     cmap(('Choice', cmap(('Word', h('f6')))))),
    ('output-union-list', 'zcbor', 'GetRecord', 200, CBOR, cmap(('Choice', h('80')))),
    ('output-struct-text', 'zcbor', 'GetRecord', 200, CBOR, cmap(('Part', text('x')))),
    ('output-numbers', 'zcbor', 'GetRecord', 200, CBOR,
     cmap(('Total', h('1bffffffffffffffff')), ('Count', h('3bffffffffffffffff')),
          ('Ratio', h('f93e00')), ('Flag', h('f4')))),
    ('output-half-specials', 'zcbor', 'GetRecord', 200, CBOR,
     cmap(('Ratio', h('f97e00')), ('Count', h('f97c00')), ('Total', h('f9fc00')),
          ('Note', h('f90001')))),
    ('output-single', 'zcbor', 'GetRecord', 200, CBOR, cmap(('Ratio', h('fa3dcccccd')))),
    ('output-timestamps', 'zcbor', 'GetRecord', 200, CBOR,
     cmap(('When', h('01')), ('Note', h('c1fb41d578be2c200000')), ('Count', h('c1f5')))),
    ('output-timestamp-bad', 'zcbor', 'GetRecord', 200, CBOR, cmap(('When', h('c16161')))),
    ('output-tag-unsupported', 'zcbor', 'GetRecord', 200, CBOR, cmap(('When', h('c001')))),
    ('output-indefinite', 'zcbor', 'GetRecord', 200, CBOR,
     h('bf') + text('Items') + h('9f') + h('7f 6161 6162 ff') + h('5f 4101 ff') + h('ff')
     + text('Data') + h('5f 4100 4101 ff') + h('ff')),
    ('output-trailing', 'zcbor', 'GetRecord', 200, CBOR, cmap(('Count', h('01'))) + h('ffff')),
    ('output-truncated', 'zcbor', 'GetRecord', 200, CBOR, h('a2') + text('Count') + h('01')),
    ('output-bad-utf8', 'zcbor', 'GetRecord', 200, CBOR, cmap(('Note', h('61ff')))),
    ('output-bad-simple', 'zcbor', 'GetRecord', 200, CBOR, cmap(('Note', h('f8')))),
    ('output-unhashable', 'zcbor', 'GetRecord', 200, CBOR, h('a1 80 01')),
    ('output-int-key-ignored', 'zcbor', 'GetRecord', 200, CBOR, h('a2 01 01') + text('Count')
     + h('02')),
    ('output-error-shape-status', 'zcbor', 'GetRecord', 300, CBOR, cmap(('Note', text('n')))),
    ('fakes-zcborqc-send-metric', 'zcborqc', 'SendMetric', 200, CBOR,
     cmap(('Echo', h('fa3dcccccd')), ('Values', h('82 fb3ff8000000000000 f93c00')),
          ('Stamps', h('82 c11a55e2f840 c1fb41d578be10200000')),
          ('Index', cmap(('a', cmap(('Name', text('n')), ('When', h('c100')))))),
          ('Payload', h('43000162')), ('Total', h('1b0000010000000000')),
          ('Enabled', h('f5')))),
    ('fakes-zcborqc-send-metric-queue-missing', 'zcborqc', 'SendMetric', 400, QC,
     cmap(('__type', text('com.example.zcborqc#QueueMissing')), ('Message', text('gone')),
          ('Queue', text('q1')))),
    ('fakes-zcborqc-send-metric-query-unmodeled', 'zcborqc', 'SendMetric', 400,
     [('x-amzn-query-error', 'AWS.Zcborqc.Other;Receiver')] + CBOR,
     cmap(('__type', text('com.example.zcborqc#Other')), ('message', text('m')))),
    ('query-error-malformed', 'zcborqc', 'SendMetric', 400,
     [('x-amzn-query-error', 'a;b;c')] + CBOR, cmap(('__type', text('X')))),
    ('query-error-empty-first', 'zcborqc', 'SendMetric', 400,
     [('x-amzn-query-error', ';Sender')] + CBOR, cmap()),
    ('fakes-zcborqc-ping', 'zcborqc', 'Ping', 200, CBOR, b''),
]


def requests():
    for row_id, service, operation, params in REQUESTS:
        service_model = model(service)
        operation_model = service_model.operation_model(operation)
        row = {'id': row_id, 'service': service, 'operation': operation, 'params': params}
        try:
            request = RpcV2CBORSerializer().serialize_to_request(decode(params), operation_model)
        except Exception as e:  # noqa: BLE001 - the error's text is the oracle
            row['error'] = str(e)
        else:
            row['serialized'] = {
                'method': request['method'],
                'url_path': request['url_path'],
                'query': request['query_string'],
                'headers': list(request['headers'].items()),
                'body_base64': base64.b64encode(request['body']).decode(),
                'host_prefix': request.get('host_prefix'),
            }
        yield row


def responses():
    def identity(value):
        return value

    for row_id, service, operation, status, headers, body in RESPONSES:
        service_model = model(service)
        operation_model = service_model.operation_model(operation)
        parser = RpcV2CBORParser(timestamp_parser=identity, blob_parser=identity)
        response = {'status_code': status, 'headers': HeadersDict(headers), 'body': body}
        row = {'id': row_id, 'service': service, 'operation': operation, 'status': status,
               'headers': headers, 'body_base64': base64.b64encode(body).decode()}
        try:
            parsed = parser.parse(response, operation_model.output_shape)
            if status >= 300:
                code = parsed.get('Error', {}).get('Code')
                shape = service_model.shape_for_error_code(code)
                if shape is not None:
                    parsed.update(parser.parse(response, shape))
        except Exception as e:  # noqa: BLE001 - the error's text is the oracle
            row['error'] = str(e)
        else:
            row['parsed'] = encode(parsed)
        yield row


json.dump({'requests': list(requests()), 'responses': list(responses())}, sys.stdout,
          indent=1, ensure_ascii=False)
sys.stdout.write('\n')
