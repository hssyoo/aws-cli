"""botocore and awscli as the parity oracle, run in the harness venv.

For every case under --cases this writes what the Python route produces:

layer A, the protocol level, against botocore's own pieces:
  - the validation report (ParamValidator), or
  - the serializer's request dict (method, url_path, query_string, headers in
    order, body, host_prefix), then for the canned response the parsed
    output under botocore's default parsers and under the CLI's two
    (cli_timestamp_format iso8601 and wire, through awscli's own handlers),
    each with awscli's JSONFormatter and TextFormatter output for the CLI
    ones, or the ClientError text with the parsed Error dict;

layer B, the call level: every prepared request a client of the CLI's own
session sends (method, URL, ordered headers with Authorization, body) for
pinned credentials, clock and uuids, and the outcome, captured in a
before-send handler that returns the canned response to every attempt.
Nothing is sent. Uuids are numbered from 1, as client::testing::FixedRuntime
numbers them, and SigV4's clock is pinned.

Only inputs are committed; this output lives under target/parity.
"""

from __future__ import annotations

import argparse
import base64
import copy
import datetime
import io
import json
import sys
import types
import uuid
from pathlib import Path

PINNED = datetime.datetime(2026, 11, 22, 21, 24, 3)
ISSUED = [0]


def decode(value):
    """The case encoding: {"$blob": base64} and {"$datetime": ISO 8601}."""
    if isinstance(value, dict):
        if set(value) == {"$blob"}:
            return base64.b64decode(value["$blob"])
        if set(value) == {"$datetime"}:
            return datetime.datetime.fromisoformat(value["$datetime"].replace("Z", "+00:00"))
        return {key: decode(item) for key, item in value.items()}
    if isinstance(value, list):
        return [decode(item) for item in value]
    return value


def dump(value):
    from awscli.utils import json_encoder

    return json.dumps(value, indent=4, ensure_ascii=False, default=json_encoder)


def service_model(case, models):
    from botocore.loaders import Loader
    from botocore.model import ServiceModel

    loader = Loader(extra_search_paths=[str(models)]) if case.get("models") == "fake" else Loader()
    description = loader.load_service_model(case["service"], "service-2")
    return ServiceModel(description, service_name=case["service"])


def request_dict(model, operation, params):
    from botocore.awsrequest import AWSRequestPreparer
    from botocore.serialize import create_serializer
    from botocore.utils import percent_encode_sequence

    serializer = create_serializer(model.resolved_protocol, include_validation=False)
    request = serializer.serialize_to_request(params, operation)
    body = request["body"]
    if isinstance(body, dict):
        # As the wire carries it: AWSRequestPreparer._prepare_body.
        body = AWSRequestPreparer()._prepare_body(types.SimpleNamespace(data=body)) or b""
        if isinstance(body, str):
            body = body.encode("utf-8")
    query = request["query_string"]
    if isinstance(query, dict):
        query = percent_encode_sequence(query)
    return {
        "method": request["method"],
        "path": request["url_path"],
        "query": query,
        "headers": [[name, value] for name, value in request["headers"].items()],
        "body": base64.b64encode(body).decode(),
        "host_prefix": request.get("host_prefix"),
    }


def response_dict(case, operation):
    from botocore.awsrequest import HeadersDict

    canned = case.get("response")
    if canned is None:
        return None
    body = canned.get("body", "").encode("utf-8")
    if "body_base64" in canned:
        body = base64.b64decode(canned["body_base64"])
    return {
        "status_code": canned.get("status", 200),
        "headers": HeadersDict(canned.get("headers", {})),
        "body": body,
        "context": {"operation_name": operation.name},
    }


def parsers():
    from awscli.customizations.timestampformat import identity, iso_format
    from botocore.parsers import ResponseParserFactory

    def factory(**defaults):
        made = ResponseParserFactory()
        made.set_parser_defaults(**defaults)
        return made

    return {
        "botocore": factory(),
        "iso8601": factory(timestamp_parser=iso_format, blob_parser=identity),
        "wire": factory(timestamp_parser=identity, blob_parser=identity),
    }


def formatted(formatter, parsed):
    """The formatter's output, or what it raised after the output it wrote."""
    stream = io.StringIO()
    try:
        formatter(types.SimpleNamespace(query=None))("command", copy.deepcopy(parsed), stream)
    except Exception as error:
        return {"written": stream.getvalue(), "error": str(error)}
    return stream.getvalue()


def outcome(name, factory, model, operation, response):
    """Endpoint._get_response's parse, the modelled error fields, and what
    BaseClient._make_api_call raises or returns."""
    from awscli.formatter import JSONFormatter, TextFormatter
    from botocore.exceptions import ClientError

    parser = factory.create_parser(model.resolved_protocol)
    try:
        parsed = parser.parse(copy.copy(response), operation.output_shape)
        if response["status_code"] >= 300:
            code = parsed.get("Error", {}).get("Code")
            shape = model.shape_for_error_code(code)
            if shape is not None:
                parsed.update(parser.parse(copy.copy(response), shape))
    except Exception as error:  # ResponseParserError and the parsers' own
        return {"parse_error": str(error)}
    if response["status_code"] >= 300:
        details = {"Error": parsed.get("Error", {})}
        details.update({k: v for k, v in parsed.items() if k not in ("Error", "ResponseMetadata")})
        details["RequestId"] = parsed.get("ResponseMetadata", {}).get("RequestId")
        return {"error": str(ClientError(parsed, operation.name)), "details": dump(details)}
    result = {"parsed": dump(parsed)}
    if name != "botocore":
        result["json"] = formatted(JSONFormatter, parsed)
        result["text"] = formatted(TextFormatter, parsed)
    return result


def layer_a(case, models):
    from botocore.exceptions import ParamValidationError
    from botocore.validate import ParamValidator

    model = service_model(case, models)
    operation = model.operation_model(case["operation"])
    params = decode(case.get("input", {}))
    vector = {}
    if operation.input_shape is not None:
        report = ParamValidator().validate(params, operation.input_shape)
        if report.has_errors():
            vector["validation"] = str(ParamValidationError(report=report.generate_report()))
            return vector
    try:
        vector["request"] = request_dict(model, operation, params)
    except Exception as error:
        vector["serialize_error"] = f"{type(error).__name__}: {error}"
        return vector
    response = response_dict(case, operation)
    if response is not None:
        vector["outcomes"] = {
            name: outcome(name, factory, model, operation, response)
            for name, factory in parsers().items()
        }
    return vector


def pin_clock_and_uuid():
    import botocore.auth as auth
    import botocore.endpoint as endpoint
    import botocore.handlers as handlers

    class Datetime(datetime.datetime):
        @classmethod
        def utcnow(cls):
            return PINNED

    auth.datetime = types.SimpleNamespace(
        datetime=Datetime, timedelta=datetime.timedelta, timezone=datetime.timezone
    )
    def uuid4():
        ISSUED[0] += 1
        return uuid.UUID(f"00000000-0000-4000-8000-{ISSUED[0]:012x}")

    pinned = types.SimpleNamespace(uuid4=uuid4)
    endpoint.uuid = pinned
    handlers.uuid = pinned
    # Retries do not wait: the endpoint's backoff sleep is a no-op, so the
    # standard mode's jitter changes nothing captured.
    endpoint.time = types.SimpleNamespace(sleep=lambda seconds: None)


class Raw:
    def __init__(self, body):
        self._body = body

    def stream(self, **kwargs):
        yield self._body


def layer_b(case, models):
    """The prepared request and the outcome of a real client call."""
    import os

    from awscli.clidriver import create_clidriver
    from botocore.awsrequest import AWSResponse
    from botocore.config import Config

    settings = case.get("settings", {})
    os.environ["AWS_ACCESS_KEY_ID"] = settings.get("access_key", "AKIDEXAMPLE")
    os.environ["AWS_SECRET_ACCESS_KEY"] = settings.get("secret_key", "secret")
    os.environ.pop("AWS_SESSION_TOKEN", None)
    if "session_token" in settings:
        os.environ["AWS_SESSION_TOKEN"] = settings["session_token"]
    ISSUED[0] = 0
    session = create_clidriver().session
    if case.get("models") == "fake":
        session.get_component("data_loader").search_paths.insert(0, str(models))
    retries = {"mode": "standard"}
    if "max_attempts" in settings:
        # Attempts in all, as the config file's max_attempts counts them
        # (botocore/config.py). Without it, a service's own default holds
        # (retries/standard.py's _SERVICE_MAX_ATTEMPTS).
        retries["max_attempts"] = settings["max_attempts"]
    config = Config(
        use_fips_endpoint=settings.get("fips", False),
        use_dualstack_endpoint=settings.get("dualstack", False),
        retries=retries,
    )
    client = session.create_client(
        case["service"],
        region_name=settings.get("region", "us-west-2"),
        endpoint_url=settings.get("endpoint_url"),
        config=config,
    )
    seen = {"requests": []}
    canned = case.get("response", {"status": 200, "headers": {}, "body": ""})

    def capture(request, **kwargs):
        body = request.body or b""
        if isinstance(body, str):
            body = body.encode()
        seen["requests"].append({
            "method": request.method,
            "url": request.url,
            "headers": [[k, v.decode() if isinstance(v, bytes) else v] for k, v in request.headers.items()],
            "body": base64.b64encode(body).decode(),
        })
        return AWSResponse(
            request.url, canned.get("status", 200), canned.get("headers", {}),
            Raw(canned.get("body", "").encode()),
        )

    client.meta.events.register("before-send", capture)
    names = {api: python for python, api in client.meta.method_to_api_mapping.items()}
    try:
        parsed = getattr(client, names[case["operation"]])(**decode(case.get("input", {})))
        seen["parsed"] = dump(parsed)
    except Exception as error:
        seen["error"] = {"class": type(error).__name__, "text": str(error)}
    return seen


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--aws-cli-source", type=Path, required=True)
    parser.add_argument("--cases", type=Path, required=True)
    parser.add_argument("--models", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    sys.path.insert(0, str(args.aws_cli_source))
    import awscli  # noqa: F401  (the botocore alias)

    pin_clock_and_uuid()
    vectors = []
    for path in sorted(args.cases.glob("*.json")):
        for case in json.loads(path.read_text()):
            vector = {"file": path.name, "case": case, "a": layer_a(case, args.models)}
            vector["b"] = layer_b(case, args.models)
            vectors.append(vector)
    args.out.write_text(json.dumps(vectors, indent=1, ensure_ascii=False))


if __name__ == "__main__":
    main()
