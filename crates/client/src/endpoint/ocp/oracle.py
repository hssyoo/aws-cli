# jmespath 1.0.1 for Python, the library botocore and the CLI import, as the
# oracle of the native engine. It runs under the toolchain's CPython, which
# has the standard library only, on the library's own source vendored in
# crates/jmespath/testdata/python (checked against the wheel's RECORD).
#
# stdin names that directory, the botocore data directory and a file of
# cases, one per line: {"expression": ..., "given": text},
# or {"expression": ..., "given": null} for one whose inputs this script
# makes from the expression's own AST. After the cases come every JMESPath
# expression the botocore models ship (paginators, waiters,
# operationContextParams), each with made inputs. For each case and input
# it writes {"kind", "expression", "given": text, "outcome"}: the result's
# repr, or "Class: str(error)".
import base64
import datetime
import glob
import hashlib
import json
import os
import sys

config = json.loads(sys.stdin.read())
root = config['python']
for line in open(os.path.join(root, 'RECORD')):
    name, digest, _ = line.strip().split(',')
    data = open(os.path.join(root, name), 'rb').read()
    actual = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b'=').decode()
    if 'sha256=' + actual != digest:
        sys.exit('vendored %s differs from the jmespath 1.0.1 wheel' % name)
sys.dont_write_bytecode = True
sys.path.insert(0, root)
import jmespath  # noqa: E402
from jmespath import parser  # noqa: E402

assert jmespath.__version__ == '1.0.1'


def decode(value):
    """{"$bytes": base64} is bytes, {"$datetime": seconds} a UTC datetime."""
    if isinstance(value, dict):
        if set(value) == {'$bytes'}:
            return base64.b64decode(value['$bytes'])
        if set(value) == {'$datetime'}:
            return datetime.datetime.fromtimestamp(value['$datetime'], datetime.timezone.utc)
        return {k: decode(v) for k, v in value.items()}
    if isinstance(value, list):
        return [decode(v) for v in value]
    return value


def outcome(expression, given):
    try:
        return repr(jmespath.search(expression, decode(json.loads(given))))
    except Exception as e:
        return '%s: %s' % (type(e).__name__, e)


def merge(a, b):
    if a is None:
        return b
    if isinstance(a, dict) and isinstance(b, dict):
        out = dict(a)
        for k, v in b.items():
            out[k] = merge(out.get(k), v)
        return out
    return a


def synth(node, leaf):
    """An input on which `node` reaches `leaf`, as far as one exists."""
    kind, children = node['type'], node.get('children', [])
    if kind == 'field':
        return {node['value']: leaf}
    if kind in ('subexpression', 'index_expression', 'pipe'):
        for child in reversed(children):
            leaf = synth(child, leaf)
        return leaf
    if kind == 'index':
        return [leaf] * min(abs(node['value']) + 1, 4)
    if kind == 'slice':
        return [leaf, leaf, leaf]
    if kind in ('projection', 'filter_projection'):
        inner = synth(children[1], leaf)
        return synth(children[0], [inner, inner])
    if kind == 'value_projection':
        inner = synth(children[1], leaf)
        return synth(children[0], {'a': inner, 'b': inner})
    if kind == 'flatten':
        return synth(children[0], [leaf, [leaf]])
    if kind in ('identity', 'current'):
        return leaf
    if kind == 'literal':
        return None
    made = None
    for child in children:
        if isinstance(child, dict):
            made = merge(made, synth(child, leaf))
    return made


LEAVES = ['v1', 7, ['a', 'b'], {'k': 'v'}, True, 2.5, None, 0, '', []]


def givens(expression):
    texts = ['{}']
    try:
        tree = parser.Parser().parse(expression).parsed
    except Exception:
        return texts
    for leaf in LEAVES:
        made = synth(tree, leaf)
        text = json.dumps(made if made is not None else leaf)
        if text not in texts:
            texts.append(text)
    return texts


def emit(kind, expression, given):
    texts = [given] if given is not None else givens(expression)
    for text in texts:
        print(json.dumps({'kind': kind, 'expression': expression, 'given': text,
                          'outcome': outcome(expression, text)}))


def model_expressions(data):
    found = {}

    def add(kind, value):
        for text in value if isinstance(value, list) else [value]:
            if isinstance(text, str):
                found.setdefault(text, kind)

    for path in sorted(glob.glob(os.path.join(data, '*', '*', 'paginators-1*.json'))):
        for config in json.load(open(path)).get('pagination', {}).values():
            for key in ('output_token', 'result_key', 'more_results', 'non_aggregate_keys'):
                add('paginator', config.get(key))
    for path in sorted(glob.glob(os.path.join(data, '*', '*', 'waiters-2.json'))):
        for waiter in json.load(open(path)).get('waiters', {}).values():
            for acceptor in waiter.get('acceptors', []):
                if acceptor.get('matcher') in ('path', 'pathAll', 'pathAny'):
                    add('waiter', acceptor.get('argument'))
    for path in sorted(glob.glob(os.path.join(data, '*', '*', 'service-2.json'))):
        text = open(path, encoding='utf-8').read()
        if 'operationContextParams' not in text:
            continue
        for operation in json.loads(text).get('operations', {}).values():
            for param in operation.get('operationContextParams', {}).values():
                add('ocp', param['path'])
    return found


for line in open(config['cases'], encoding='utf-8'):
    case = json.loads(line)
    emit(case.get('kind', 'case'), case['expression'], case['given'])
for expression, kind in model_expressions(config['data']).items():
    emit(kind, expression, None)
