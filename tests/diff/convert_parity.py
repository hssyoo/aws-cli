"""Convert the parity cases into the differential corpus, mechanically.

Each case of `tests/parity/cases/*.json` (read, never written) becomes one
row of `corpus/parity.jsonl`, in file then case order: the invocation a
user would type for the case's operation and input on the in-tree CLI.

- argv: the service, the operation and one argument per input member,
  each named as the CLI names it (botocore's `xform_name`, joined by `-`).
  A boolean member is `--name` or `--no-name`; a scalar is its text; a
  structure, list, map, union or document is compact JSON; the case
  encoding's `{"$blob": ...}` is its base64 text (the CLI's default binary
  format) and `{"$datetime": ...}` its ISO 8601 text.
- settings: `region` is `--region`; `fips`, `dualstack`, `max_attempts`,
  `access_key`, `secret_key` and `session_token` are the matching AWS_*
  variables. `paginate`'s `MaxItems`, `PageSize` and `StartingToken` are
  `--max-items`, `--page-size` and `--starting-token`. A case without
  `paginate` is one call, so an operation the CLI would page gets
  `--no-paginate`, which also gives a member the CLI's paging options
  shadow (iam's `MaxItems`) its own option back.
- replies: `responses`, or the one `response`, as they are; headers keep
  their order. A fake-model case reads its models from `tests/models`.

What the CLI would read otherwise fails the conversion: a blob not in the
case encoding, a value argparse takes for an option (a leading `-`) or the
CLI for a file (`file://`, `fileb://`), a member a paging option shadows
while paging, an unknown setting and a streaming output. What makes the CLI
stop before botocore's validator, or change a value's type, is named in
the row's `source` after `; argparse: `: a member the model lacks or a
required one the case leaves out (a validation case), either of which
masks the case's other checks, and a top-level scalar of the wrong type.

Then every row runs, in process, on the in-tree CLI, each request answered
in turn by the row's replies, and must send what the parity oracle's call
of its case sends (`tests/parity/oracle.py`'s layer B): the same method,
URL, headers (but the User-Agent) and body, request for request, the order
of query pairs, form pairs, headers and JSON members and the case of
header names aside. A member that does not reach the wire,
or a call the CLI pages where the case makes one, fails the conversion.
The check is attacked first: it must tell apart a dropped or changed
header, query, label, host label, body or payload member (`attack`), and
for every row, each input member dropped or altered in its case must be
seen wherever it changes what botocore's call sends (`mutation_misses`),
also on `ATTACK_CASES`, which bind members where no parity case does.

`--out FILE` writes the corpus; `--check FILE` fails unless FILE is what
the converter writes.
"""

from __future__ import annotations

import argparse
import base64
import json
import re
import sys
from pathlib import Path
from typing import Any, Final

FAKE_MODELS: Final = "tests/models"
SETTINGS: Final = {
    "fips": "AWS_USE_FIPS_ENDPOINT",
    "dualstack": "AWS_USE_DUALSTACK_ENDPOINT",
    "max_attempts": "AWS_MAX_ATTEMPTS",
    "access_key": "AWS_ACCESS_KEY_ID",
    "secret_key": "AWS_SECRET_ACCESS_KEY",
    "session_token": "AWS_SESSION_TOKEN",
}
PAGINATION: Final = {
    "MaxItems": "--max-items",
    "PageSize": "--page-size",
    "StartingToken": "--starting-token",
}


def wire(value: Any) -> Any:
    """The case encoding decoded to what the CLI's JSON input takes."""
    if isinstance(value, dict):
        if set(value) == {"$blob"}:
            return value["$blob"]
        if set(value) == {"$datetime"}:
            return value["$datetime"]
        return {key: wire(item) for key, item in value.items()}
    if isinstance(value, list):
        return [wire(item) for item in value]
    return value


def text(value: Any) -> str:
    value = wire(value)
    if isinstance(value, str):
        return value
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False)


SCALARS: Final = {
    "string": (str,),
    "integer": (int,),
    "long": (int,),
    "float": (int, float),
    "double": (int, float),
    "boolean": (bool,),
}
PAGING_FLAGS: Final = frozenset(flag.lstrip("-") for flag in PAGINATION.values())
FILE_PREFIXES: Final = ("file://", "fileb://")


def arguments(
    name: str, operation: Any, params: dict[str, Any], xform_name: Any, paging: bool
) -> tuple[list[str], list[str]]:
    """The argv for `params`, and what makes argparse stop or retype it."""
    members = operation.input_shape.members if operation.input_shape else {}
    argv: list[str] = []
    masks: list[str] = []
    for member_name, value in params.items():
        flag = xform_name(member_name, "-")
        member = members.get(member_name)
        if member is None:
            masks.append(f"--{flag} is not an option")
        elif paging and flag in PAGING_FLAGS:
            raise SystemExit(f"{name}: the paging option --{flag} shadows the member {member_name}")
        elif member.type_name in SCALARS and (
            not isinstance(value, SCALARS[member.type_name])
            or (isinstance(value, bool) and member.type_name != "boolean")
        ):
            article = "an" if member.type_name[0] in "aeiou" else "a"
            masks.append(f"--{flag} takes {article} {member.type_name}, not {json.dumps(value)}")
        elif member.type_name == "blob" and not (isinstance(value, dict) and set(value) == {"$blob"}):
            raise SystemExit(f"{name}: the blob {member_name} is not in the case encoding")
        if member is not None and member.type_name == "boolean" and isinstance(value, bool):
            argv.append(f"--{flag}" if value else f"--no-{flag}")
            continue
        argument = text(value)
        if argument.startswith("-") or argument.lower().startswith(FILE_PREFIXES):
            raise SystemExit(f"{name}: the CLI reads --{flag} {argument} as another option or a file")
        argv += [f"--{flag}", argument]
    required = operation.input_shape.required_members if operation.input_shape else []
    masks += [f"--{xform_name(name, '-')} is required" for name in required if name not in params]
    return argv, masks


def settings(case: dict[str, Any]) -> tuple[list[str], dict[str, str]]:
    argv: list[str] = []
    env: dict[str, str] = {}
    for name, value in case.get("settings", {}).items():
        if name == "region":
            argv += ["--region", value]
        elif name in SETTINGS:
            env[SETTINGS[name]] = str(value).lower() if isinstance(value, bool) else str(value)
        else:
            raise SystemExit(f"{case['name']}: no conversion for setting {name}")
    for name, value in case.get("paginate", {}).items():
        if name not in PAGINATION:
            raise SystemExit(f"{case['name']}: no conversion for paginate {name}")
        argv += [PAGINATION[name], str(value)]
    return argv, env


def reply(canned: dict[str, Any]) -> dict[str, Any]:
    spec: dict[str, Any] = {
        "status": canned.get("status", 200),
        "headers": [[name, value] for name, value in canned.get("headers", {}).items()],
    }
    for key in ("body", "body_base64"):
        if key in canned:
            spec[key] = canned[key]
    return spec


def row(case: dict[str, Any], source: str, session: Any, xform_name: Any) -> dict[str, Any]:
    from awscli.customizations.paginate import get_paginator_config

    model = session.get_service_model(case["service"])
    operation = model.operation_model(case["operation"])
    if operation.has_streaming_output:
        raise SystemExit(f"{case['name']}: a streaming output needs an outfile argument")
    extra_argv, env = settings(case)
    paging = get_paginator_config(session, case["service"], case["operation"]) is not None
    if paging and "paginate" not in case:
        extra_argv.append("--no-paginate")
        paging = False
    argv = [case["service"], xform_name(case["operation"], "-")]
    members, masks = arguments(case["name"], operation, case.get("input", {}), xform_name, paging)
    argv += members + extra_argv
    if masks:
        source += "; argparse: " + "; ".join(masks)
    replies = case.get("responses") or ([case["response"]] if "response" in case else [])
    return {
        "id": case["name"],
        "source": source,
        "argv": argv,
        "env": env,
        "data_path": FAKE_MODELS if case.get("models") == "fake" else None,
        "responses": [reply(canned) for canned in replies],
    }


def corpus(source: Path, cases: Path, models: Path) -> str:
    sys.path.insert(0, str(source))
    import awscli  # noqa: F401  (aliases botocore to the in-tree copy)
    from botocore import xform_name
    from botocore.session import get_session

    session = get_session()
    session.get_component("data_loader").search_paths.insert(0, str(models))
    converted = []
    for path in sorted(cases.glob("*.json")):
        for case in json.loads(path.read_text(encoding="utf-8")):
            where = f"tests/parity/cases/{path.name}#{case['name']}"
            converted.append((case, row(case, where, session, xform_name)))
    wire = Wire(cases.parent / "oracle.py", models)
    attack()
    attacked = [(case, row(case, "attack", session, xform_name)) for case in ATTACK_CASES]
    wrong = []
    for case, line in converted + attacked:
        sent, expected = wire.cli(line), wire.oracle(case)
        if found := difference(sent, expected):
            wrong.append(f"{line['id']}: {found}")
        wrong += [f"{line['id']}: {miss}" for miss in mutation_misses(wire, case, sent, expected)]
    if wrong:
        raise SystemExit("rows that do not send their case's requests:\n" + "\n".join(wrong))
    lines = [json.dumps(line, ensure_ascii=False) for _, line in converted]
    return "".join(f"{line}\n" for line in lines)


Sent = tuple[str, str, tuple[tuple[str, str], ...], bytes]


def requests_sent(argv: list[str], env: dict[str, str], replies: list[dict[str, Any]]) -> list[Sent]:
    """What the in-tree CLI sends for `argv` under `env`, in process, each
    request answered by the next of `replies` (the last repeating)."""
    import contextlib
    import io
    import os

    from awscli.clidriver import create_clidriver
    from botocore.awsrequest import AWSResponse

    sent: list[Sent] = []

    def answer(request: Any, **kwargs: Any) -> Any:
        sent.append(sending(request.method, request.url, request.headers.items(), request.body))
        spec = replies[min(len(sent), len(replies)) - 1] if replies else {"status": 200, "body": ""}
        raw = base64.b64decode(spec["body_base64"]) if "body_base64" in spec else spec.get("body", "").encode()
        return AWSResponse(request.url, spec["status"], dict(spec.get("headers", [])), Raw(raw))

    saved = dict(os.environ)
    try:
        # awscli's import put its own data on AWS_DATA_PATH; keep it last.
        data_path = os.environ.get("AWS_DATA_PATH", "")
        for name in [name for name in os.environ if name.startswith("AWS_")]:
            del os.environ[name]
        os.environ.update(env)
        os.environ["AWS_DATA_PATH"] = os.pathsep.join(
            path for path in (env.get("AWS_DATA_PATH"), data_path) if path
        )
        driver = create_clidriver()
        driver.session.register("before-send", answer)
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            driver.main(argv)
    finally:
        os.environ.clear()
        os.environ.update(saved)
    return sent


def sending(method: str, url: str, headers: Any, body: Any) -> Sent:
    as_text = [(name, value.decode() if isinstance(value, bytes) else value) for name, value in headers]
    body = body or b""
    return method, url, tuple(as_text), body.encode() if isinstance(body, str) else body


class Raw:
    def __init__(self, body: bytes) -> None:
        self._body = body

    def stream(self, **kwargs: Any) -> Any:
        yield self._body


class Wire:
    """The requests a case's call (the parity oracle's layer B) and a row's
    CLI run send, under the oracle's pinned clock and uuids."""

    def __init__(self, oracle_path: Path, models: Path) -> None:
        import importlib.util

        spec = importlib.util.spec_from_file_location("parity_oracle", oracle_path)
        if spec is None or spec.loader is None:
            raise SystemExit(f"cannot load {oracle_path}")
        self.module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.module)
        self.module.pin_clock_and_uuid()
        self.models = models

    def oracle(self, case: dict[str, Any]) -> list[Sent]:
        import os

        saved = dict(os.environ)
        try:
            self.module.ISSUED[0] = 0
            requests = self.module.layer_b(case, self.models)["requests"]
        finally:
            os.environ.clear()
            os.environ.update(saved)
        return [
            sending(request["method"], request["url"], request["headers"], base64.b64decode(request["body"]))
            for request in requests
        ]

    def cli(self, line: dict[str, Any]) -> list[Sent]:
        import os
        import tempfile

        with tempfile.TemporaryDirectory() as home:
            env = {
                "HOME": home,
                "AWS_CONFIG_FILE": os.path.join(home, "config"),
                "AWS_SHARED_CREDENTIALS_FILE": os.path.join(home, "credentials"),
                "AWS_EC2_METADATA_DISABLED": "true",
                "AWS_PAGER": "",
                "AWS_ACCESS_KEY_ID": "AKIDEXAMPLE",
                "AWS_SECRET_ACCESS_KEY": "secret",
                "AWS_DEFAULT_REGION": "us-west-2",
                **line["env"],
            }
            if line["data_path"] is not None:
                env["AWS_DATA_PATH"] = str(self.models)
            self.module.ISSUED[0] = 0
            return requests_sent(line["argv"], env, line["responses"])


def difference(sent: list[Sent], expected: list[Sent]) -> str:
    """How `sent` differs from what the case's call sends, or ""."""
    if len(sent) != len(expected):
        return f"{len(sent)} requests, where the case's call sends {len(expected)}"
    for index, (got, want) in enumerate(zip(sent, expected)):
        if unordered(*got) != unordered(*want):
            return f"request {index} is {got!r}, where the case's call sends {want!r}"
    return ""


# Headers only the route sets: the CLI names itself in its User-Agent.
ROUTE_HEADERS: Final = frozenset({"user-agent"})


def unordered(method: str, url: str, headers: tuple[tuple[str, str], ...], body: bytes) -> tuple[Any, ...]:
    """A request with its query pairs, form pairs, headers and JSON members
    in any order (the CLI serializes a structure in its model's order, a
    call in its arguments'), header names in any case, and every number as
    its text, so `1` is neither `1.0` nor `true`. A signature is over those
    bytes, so only its scope and signed headers are compared."""
    origin, mark, query = url.partition("?")
    if body.startswith(b"{"):
        shape: Any = json.loads(
            body,
            parse_int=lambda text: ("int", text),
            parse_float=lambda text: ("float", text),
            object_pairs_hook=lambda pairs: sorted(pairs, key=repr),
        )
    elif b"=" in body and not body.startswith(b"<"):
        shape = sorted(body.split(b"&"))
    else:
        shape = body
    named = sorted(
        (name.lower(), re.sub(r"Signature=[0-9a-f]{64}", "Signature=<sig>", value))
        for name, value in headers
        if name.lower() not in ROUTE_HEADERS
    )
    return method, origin, mark, sorted(query.split("&")), named, shape


def altered(value: Any) -> Any:
    """`value` changed in its last scalar, or None when nothing is left to
    change: what a converter that mangles a value would send."""
    import datetime

    if isinstance(value, bool):
        return not value
    if isinstance(value, (int, float)):
        return value + 1
    if isinstance(value, str):
        return value[:-1] + ("y" if value.endswith("x") else "x")
    if isinstance(value, dict) and set(value) == {"$blob"}:
        return {"$blob": base64.b64encode(base64.b64decode(value["$blob"]) + b"!").decode()}
    if isinstance(value, dict) and set(value) == {"$datetime"}:
        moment = datetime.datetime.fromisoformat(value["$datetime"].replace("Z", "+00:00"))
        return {"$datetime": (moment + datetime.timedelta(seconds=1)).isoformat()}
    if isinstance(value, dict) and value:
        key = next(iter(value))
        inner = altered(value[key])
        return None if inner is None else {**value, key: inner}
    if isinstance(value, list) and value:
        inner = altered(value[-1])
        return None if inner is None else [*value[:-1], inner]
    return None


def mutation_misses(wire: Wire, case: dict[str, Any], sent: list[Sent], expected: list[Sent]) -> list[str]:
    """The wire check attacked: each input member dropped, then altered, in
    the case. Wherever that changes what botocore's call sends (every byte,
    signature and all), the row's unchanged run must differ from it."""
    params = case.get("input", {})
    misses = []
    for name, value in params.items():
        changed = altered(value)
        variants = [("dropped", {key: item for key, item in params.items() if key != name})]
        if changed is not None:
            variants.append(("altered", {**params, name: changed}))
        for how, mutated in variants:
            truth = wire.oracle({**case, "input": mutated})
            if truth != expected and not difference(sent, truth):
                misses.append(f"the wire check misses {name} {how}")
    return misses


def signed(region: str, headers: str, digit: str) -> tuple[str, str]:
    scope = f"AKIDEXAMPLE/20261122/{region}/zjson/aws4_request"
    return ("Authorization", f"AWS4-HMAC-SHA256 Credential={scope}, SignedHeaders={headers}, Signature={digit * 64}")


# Cases the wire check runs and attacks, never written: a binding no parity
# case has yet, an input member bound to the payload.
ATTACK_CASES: Final = [
    {
        "name": "attack-iot-data-publish-payload",
        "service": "iot-data",
        "operation": "Publish",
        "input": {"topic": "t/1", "qos": 1, "payload": {"$blob": "aGk="}},
        "response": {"status": 200, "headers": {}, "body": ""},
    },
]


def attack() -> None:
    """Requests the comparison must tell apart from a base request: each a
    way a converted row could go wrong and still look right."""
    base: Sent = (
        "POST",
        "https://z.data.zjson.us-west-2.amazonaws.com/p/l?a=1&b=2",
        (("X-Trace", "t"), ("X-Meta-k", "v"), ("Content-Type", "application/json"), ("User-Agent", "u")),
        b'{"N":1,"F":1.5,"B":true,"S":"s"}',
    )
    method, url, headers, body = base
    bare = (method, url.split("?")[0], headers, body)
    evasions: dict[str, Sent] = {
        "a header member dropped": (method, url, headers[1:], body),
        "a header member changed": (method, url, (("X-Trace", "u"), *headers[1:]), body),
        "a prefix-map header changed": (method, url, (headers[0], ("X-Meta-k", "w"), *headers[2:]), body),
        "a header the call does not send": (method, url, (*headers, ("X-Extra", "1")), body),
        "a header repeated": (method, url, (*headers, headers[0]), body),
        "a query member changed": (method, url.replace("b=2", "b=3"), headers, body),
        "a query member dropped": (method, url.replace("&b=2", ""), headers, body),
        "an empty query": (method, url.split("?")[0] + "?", headers, body),
        "a label member changed": (method, url.replace("/l?", "/m?"), headers, body),
        "a host label changed": (method, url.replace("z.data", "y.data"), headers, body),
        "an integer sent as a float": (method, url, headers, body.replace(b'"N":1', b'"N":1.0')),
        "an integer sent as a boolean": (method, url, headers, body.replace(b'"N":1', b'"N":true')),
        "a body member dropped": (method, url, headers, body.replace(b',"S":"s"', b"")),
        "a payload changed": (method, url, headers, b"raw"),
        "another method": ("PUT", url, headers, body),
        "another signing scope": (method, url, (*headers, signed("us-east-1", "x-trace", "0")), body),
        "other signed headers": (method, url, (*headers, signed("us-west-2", "host", "0")), body),
    }
    if difference([base], [base]):
        raise SystemExit("the wire check tells a request from itself")
    reordered = (("x-trace", "t"), *reversed(headers[1:-1]), ("User-Agent", "v"))
    benign = (method, "https://z.data.zjson.us-west-2.amazonaws.com/p/l?b=2&a=1", reordered, body)
    if difference([benign], [base]):
        raise SystemExit("the wire check tells order, name case or the User-Agent apart")
    resigned = (method, url, (*headers, signed("us-west-2", "x-trace", "1")), body)
    signed_base = (method, url, (*headers, signed("us-west-2", "x-trace", "0")), body)
    if difference([resigned], [signed_base]):
        raise SystemExit("the wire check tells two signatures of one scope apart")
    passed = [
        how
        for how, request in evasions.items()
        if not difference([request], [signed_base if "sign" in how else base])
    ]
    if not difference([evasions["an empty query"]], [bare]):
        passed.append("an empty query against none")
    if passed:
        raise SystemExit("the wire check misses " + ", ".join(passed))


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--aws-cli-source", type=Path, required=True)
    parser.add_argument("--cases", type=Path, required=True)
    parser.add_argument("--models", type=Path, required=True)
    target = parser.add_mutually_exclusive_group(required=True)
    target.add_argument("--out", type=Path)
    target.add_argument("--check", type=Path)
    args = parser.parse_args()
    written = corpus(args.aws_cli_source.resolve(), args.cases.resolve(), args.models.resolve())
    if args.out is not None:
        args.out.write_text(written, encoding="utf-8")
        return
    if args.check.read_text(encoding="utf-8") != written:
        raise SystemExit(
            f"{args.check} is not what tests/diff/convert_parity.py writes; "
            "regenerate it with --out and review the change"
        )


if __name__ == "__main__":
    main()
