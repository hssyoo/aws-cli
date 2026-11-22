# Prints botocore's ParamValidator report for each case.
# Run from the repository root after any blackbox run:
#   target/blackbox/repo/venv/bin/python crates/model/src/validate/oracle.py \
#       crates/model/src/validate/fixture.json > crates/model/src/validate/expected.json
import datetime, json, sys
sys.path.insert(0, 'aws-cli')
import awscli  # noqa: F401  (installs the botocore alias)
from botocore.model import ServiceModel
from botocore.validate import ParamValidator

model = ServiceModel(json.load(open(sys.argv[1])))
shape = model.operation_model('Check').input_shape
utc = datetime.timezone.utc
ok = {'Req': 'abc', 'Nested': {'X': 'x'}}
cases = {
    'missing': {},
    'unknown': {'Req': 'abc', 'Nested': {'X': 'a'}, 'Bogus': 1, 'Other': 2},
    'types': {'Req': 5, 'Nested': [], 'Num': '1', 'Flag': 'true', 'Opt': None, 'List': 'a',
              'Map': [], 'Dbl': 'x', 'Long': 1.5},
    'ranges': {'Req': 'ab', 'Nested': {'X': 'x'}, 'Num': 0, 'Long': 9, 'Dbl': 0.25, 'List': [],
               'Fmin': 1},
    'bools_are_ints': dict(ok, Num=True, Dbl=False, Long=True),
    'map': dict(ok, Map={'k': 1, 'kk': 'x', 'kkk': 0}),
    'list': dict(ok, List=['a', 5, ['x'], 'abc']),
    'union_empty': dict(ok, Union={}),
    'union_two': dict(ok, Union={'A': 'abc', 'B': 1}),
    'document': dict(ok, Doc={'a': [1, {'b': b'x'}], 'c': datetime.datetime(2020, 1, 2, tzinfo=utc),
                              'd': None, 'e': 1.5}),
    'document_top': dict(ok, Doc=b'raw'),
    'jsonvalue': dict(ok, Json={'a': b'x'}),
    'jsonvalue_datetime': dict(ok, Json=[datetime.datetime(2020, 1, 2, tzinfo=utc)]),
    'jsonvalue_ok': dict(ok, Json={'a': [1, None]}),
    'blob': dict(ok, Blob=5),
    'blob_ok': dict(ok, Blob='text'),
    'timestamps_bad': dict(ok, When=[1]),
    'timestamp_none': dict(ok, When=None),
    'timestamp_nan': dict(ok, When=float('nan')),
    'timestamps_ok': dict(ok, When='12'),
    'host_label': dict(ok, Host=''),
    'nested': {'Req': 'abc', 'Nested': {'Z': 1, 'Y': 'y'}},
    'not_a_dict': [],
    'repr': dict(ok, Opt={"it's": ['a\nb', 'q"\'', 2.5e-7]}),
    'big_negative': dict(ok, Num=-99999999999999999999),
    'big_positive': dict(ok, Num=99999999999999999999),
    'float_for_int': dict(ok, Num=1.0),
    'non_ascii': dict(ok, Req='é😀', Opt=['é😀\x00']),
    'everything': {'Bogus': 1, 'Num': 0, 'Req': 1, 'List': [1], 'Union': {}},
}
out = {}
for name, params in cases.items():
    report = ParamValidator().validate(params, shape)
    out[name] = report.generate_report() if report.has_errors() else None
json.dump(out, sys.stdout, indent=1, ensure_ascii=False)
