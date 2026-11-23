# botocore's request for each case on stdin, as JSON on stdout: the
# prepared request (`AWSPreparedRequest`) of a real client's call, signed
# with the case's credentials at the case's instant, as `before-send` sees
# it. The send is stopped there; nothing goes out.
#
# uuid4 is numbered from 1, as the native tests' fixed runtime numbers it,
# and SigV4's clock is pinned (`datetime.datetime.utcnow` in
# botocore.auth). Like the endpoint oracle, it runs under the toolchain's
# CPython with stand-ins for botocore's third-party imports.
import importlib.abc
import importlib.machinery
import datetime
import json
import os
import sys
import types
import uuid

STAND_INS = ('jmespath', 'dateutil', 'urllib3', 'awscrt')


class Anything(type):
    """A class whose every attribute is another such class."""

    def __getattr__(cls, name):
        if name.startswith('__'):
            raise AttributeError(name)
        value = Anything(name, (Warning,), {'__init__': lambda self, *a, **k: None})
        setattr(cls, name, value)
        return value


class StandIn(types.ModuleType):
    def __getattr__(self, name):
        if name.startswith('__'):
            raise AttributeError(name)
        value = Anything(name, (Warning,), {'__init__': lambda self, *a, **k: None})
        setattr(self, name, value)
        return value


class Finder(importlib.abc.MetaPathFinder, importlib.abc.Loader):
    def find_spec(self, name, path, target=None):
        if name.split('.')[0] in STAND_INS:
            return importlib.machinery.ModuleSpec(name, self, is_package=True)
        return None

    def create_module(self, spec):
        module = StandIn(spec.name)
        module.__path__ = []
        return module

    def exec_module(self, module):
        pass


sys.meta_path.insert(0, Finder())
sys.path.insert(0, sys.argv[1])
for name in ('AWS_ENDPOINT_URL', 'AWS_REGION', 'AWS_DEFAULT_REGION', 'AWS_PROFILE'):
    os.environ.pop(name, None)
os.environ['AWS_CONFIG_FILE'] = os.devnull
os.environ['AWS_SHARED_CREDENTIALS_FILE'] = os.devnull

import awscli  # noqa: E402,F401  (installs the botocore alias)
import botocore.httpsession  # noqa: E402
import botocore.session  # noqa: E402
from botocore.config import Config  # noqa: E402

botocore.httpsession.URLLib3Session.__init__ = lambda self, *a, **k: None




class Stopped(Exception):
    pass


def pin(epoch):
    import botocore.auth

    class Pinned(datetime.datetime):
        @classmethod
        def utcnow(cls):
            return datetime.datetime.fromtimestamp(epoch, datetime.timezone.utc).replace(tzinfo=None)

    botocore.auth.datetime = types.SimpleNamespace(datetime=Pinned)
    issued = [0]

    def uuid4():
        issued[0] += 1
        return uuid.UUID(f'00000000-0000-4000-8000-{issued[0]:012x}')

    uuid.uuid4 = uuid4


def request(case):
    session = botocore.session.get_session()
    keys = case['credentials']
    session.set_credentials(keys['access_key_id'], keys['secret_access_key'], keys.get('session_token'))
    pin(case['epoch'])
    client = session.create_client(case['service'], region_name=case['region'])
    captured = {}

    def capture(request, **kwargs):
        body = request.body
        if isinstance(body, str):
            body = body.encode()
        captured.update(
            method=request.method,
            url=request.url,
            headers=[[name, value.decode() if isinstance(value, bytes) else value]
                     for name, value in request.headers.items()],
            body=(body or b'').decode(),
        )
        raise Stopped()

    client.meta.events.register('before-send', capture)
    try:
        getattr(client, case['method'])(**case['params'])
    except Stopped:
        pass
    return captured


def main():
    json.dump([request(case) for case in json.load(sys.stdin)], sys.stdout)


main()
