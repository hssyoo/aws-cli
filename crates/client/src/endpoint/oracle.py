# botocore's endpoint resolution for each case on stdin, as JSON on stdout.
#
# For each case it creates a real client (`Session.create_client`, which
# normalizes FIPS pseudo-regions, applies the region rules of
# `ClientArgsCreator._build_endpoint_resolver` and computes the built-ins)
# and resolves the operation's endpoint with `EndpointRulesetResolver`,
# then `auth_schemes_to_signing_ctx` for the signing context. Nothing is
# sent.
#
# It runs under the toolchain's CPython, which has the standard library
# only: botocore's third-party imports, which this path imports but never
# calls, are given empty stand-ins, and the HTTP session it would build for
# sending is not built.
import importlib.abc
import importlib.machinery
import json
import os
import sys
import types

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


def resolve(session, case):
    config = Config(
        use_fips_endpoint=case.get('fips'),
        use_dualstack_endpoint=case.get('dualstack'),
    )
    client = session.create_client(
        case['service'],
        region_name=case.get('region'),
        endpoint_url=case.get('endpoint_url'),
        config=config,
    )
    resolver = client._ruleset_resolver
    model = client.meta.service_model.operation_model(case['operation'])
    endpoint = resolver.construct_endpoint(model, {}, {})
    schemes = endpoint.properties.get('authSchemes')
    signing = resolver.auth_schemes_to_signing_ctx(schemes)[1] if schemes else {}
    return {
        'url': endpoint.url,
        'signing_region': signing.get('region'),
        'signing_name': signing.get('signing_name'),
    }


def main():
    session = botocore.session.get_session()
    session.set_credentials('AKID', 'SECRET')
    results = []
    for case in json.load(sys.stdin):
        try:
            results.append(resolve(session, case))
        except Exception as error:  # noqa: BLE001  (the error is the result)
            results.append({'error': type(error).__name__, 'message': str(error)})
    json.dump(results, sys.stdout)


main()
