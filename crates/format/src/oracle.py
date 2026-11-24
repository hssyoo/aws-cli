# The Python side of format's differential test: awscli's own formatters
# and timestamp parser over the corpus on stdin, and TextFormatter over
# paginated responses. Run by src/tests.rs with the blackbox venv's python,
# from the repository root.
import base64, datetime, io, json, sys, types

sys.path.insert(0, 'aws-cli')
import jmespath  # noqa: E402
from awscli.customizations.timestampformat import iso_format  # noqa: E402
from awscli.formatter import JSONFormatter, TextFormatter  # noqa: E402
from botocore.paginate import PageIterator  # noqa: E402

UTC = datetime.timezone.utc


def decode(value):
    if isinstance(value, dict):
        if set(value) == {'$bytes'}:
            return base64.b64decode(value['$bytes'])
        if set(value) == {'$datetime'}:
            epoch = datetime.datetime(1970, 1, 1, tzinfo=UTC)
            return epoch + datetime.timedelta(microseconds=value['$datetime'])
        return {key: decode(item) for key, item in value.items()}
    if isinstance(value, list):
        return [decode(item) for item in value]
    return value


def formatted(formatter, response):
    stream = io.StringIO()
    formatter(types.SimpleNamespace(query=None))('command', response, stream)
    return stream.getvalue()


class Pages(PageIterator):
    """A case's pages as a PageIterator yields them, the non-aggregate part
    recorded from the first, then the case's failure."""

    def __init__(self, case):
        self._result_keys = [jmespath.compile(k) for k in case['result_keys']]
        self._non_aggregate_key_exprs = [
            jmespath.compile(k) for k in case['non_aggregate_keys']
        ]
        self._non_aggregate_part = {}
        self._resume_token = case['resume_token']
        self._case = case

    def __iter__(self):
        for index, page in enumerate(decode(self._case['pages'])):
            if index == 0:
                self._record_non_aggregate_key_values(page)
            yield page
        if self._case['fail'] is not None:
            raise RuntimeError(self._case['fail'])


def paged(case):
    stream = io.StringIO()
    try:
        TextFormatter(types.SimpleNamespace(query=None))(
            'command', Pages(case), stream
        )
    except Exception as error:
        return {'error': str(error), 'written': stream.getvalue()}
    return {'ok': stream.getvalue()}


corpus = json.loads(sys.stdin.read())
out = {'responses': [], 'timestamps': [], 'pages': []}
for response in corpus['responses']:
    row = []
    for formatter in (JSONFormatter, TextFormatter):
        try:
            row.append({'ok': formatted(formatter, decode(response))})
        except Exception as error:
            row.append({'error': str(error)})
    out['responses'].append(row)
for case in corpus['pages']:
    out['pages'].append(paged(case))
for wire in corpus['timestamps']:
    try:
        out['timestamps'].append({'ok': iso_format(wire)})
    except Exception as error:
        out['timestamps'].append({'error': type(error).__name__})
sys.stdout.write(json.dumps(out))
