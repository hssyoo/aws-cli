# The Python side of format's differential test: awscli's own formatters
# and timestamp parser over the corpus on stdin. Run by src/tests.rs with
# the blackbox venv's python, from the repository root.
import base64, datetime, io, json, sys, types

sys.path.insert(0, 'aws-cli')
from awscli.customizations.timestampformat import iso_format  # noqa: E402
from awscli.formatter import JSONFormatter, TextFormatter  # noqa: E402

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


corpus = json.loads(sys.stdin.read())
out = {'responses': [], 'timestamps': []}
for response in corpus['responses']:
    row = []
    for formatter in (JSONFormatter, TextFormatter):
        try:
            row.append({'ok': formatted(formatter, decode(response))})
        except Exception as error:
            row.append({'error': str(error)})
    out['responses'].append(row)
for wire in corpus['timestamps']:
    try:
        out['timestamps'].append({'ok': iso_format(wire)})
    except Exception as error:
        out['timestamps'].append({'error': type(error).__name__})
sys.stdout.write(json.dumps(out))
