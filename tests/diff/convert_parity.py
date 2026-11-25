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
URL and body, request for request, the order of query pairs, form pairs
and JSON members aside. A member that does not reach the wire,
or a call the CLI pages where the case makes one, fails the conversion.

`--out FILE` writes the corpus; `--check FILE` fails unless FILE is what
the converter writes.
"""

from __future__ import annotations

import argparse
import base64
import json
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
    wrong = [
        f"{line['id']}: {difference}"
        for case, line in converted
        if (difference := on_the_wire(case, line, cases.parent / "oracle.py", models))
    ]
    if wrong:
        raise SystemExit("rows that do not send their case's requests:\n" + "\n".join(wrong))
    lines = [json.dumps(line, ensure_ascii=False) for _, line in converted]
    return "".join(f"{line}\n" for line in lines)


def requests_sent(argv: list[str], env: dict[str, str], replies: list[dict[str, Any]]) -> list[Any]:
    """What the in-tree CLI sends for `argv` under `env`, in process, each
    request answered by the next of `replies` (the last repeating)."""
    import contextlib
    import io
    import os

    from awscli.clidriver import create_clidriver
    from botocore.awsrequest import AWSResponse

    sent: list[Any] = []

    def answer(request: Any, **kwargs: Any) -> Any:
        body = request.body or b""
        sent.append((request.method, request.url, body.encode() if isinstance(body, str) else body))
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


class Raw:
    def __init__(self, body: bytes) -> None:
        self._body = body

    def stream(self, **kwargs: Any) -> Any:
        yield self._body


def on_the_wire(case: dict[str, Any], line: dict[str, Any], oracle_path: Path, models: Path) -> str:
    """How `line`'s requests differ from what its case's call sends, or ""."""
    import importlib.util
    import os
    import tempfile

    spec = importlib.util.spec_from_file_location("parity_oracle", oracle_path)
    oracle = sys.modules.get("parity_oracle")
    if oracle is None and spec is not None and spec.loader is not None:
        oracle = importlib.util.module_from_spec(spec)
        sys.modules["parity_oracle"] = oracle
        spec.loader.exec_module(oracle)
        oracle.pin_clock_and_uuid()
    saved = dict(os.environ)
    try:
        oracle.ISSUED[0] = 0
        expected = [
            (request["method"], request["url"], base64.b64decode(request["body"]))
            for request in oracle.layer_b(case, models)["requests"]
        ]
    finally:
        os.environ.clear()
        os.environ.update(saved)
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
            env["AWS_DATA_PATH"] = str(models)
        oracle.ISSUED[0] = 0
        sent = requests_sent(line["argv"], env, line["responses"])
    if len(sent) != len(expected):
        return f"{len(sent)} requests, where the case's call sends {len(expected)}"
    for index, (got, want) in enumerate(zip(sent, expected)):
        if unordered(*got) != unordered(*want):
            return f"request {index} is {got!r}, where the case's call sends {want!r}"
    return ""


def unordered(method: str, url: str, body: bytes) -> tuple[Any, ...]:
    """A request with its query pairs, form pairs and JSON members in any
    order: the CLI serializes a structure in its model's order, a call in
    its arguments'."""
    origin, _, query = url.partition("?")
    if body.startswith(b"{"):
        shape: Any = json.loads(body)
    elif b"=" in body and not body.startswith(b"<"):
        shape = sorted(body.split(b"&"))
    else:
        shape = body
    return method, origin, sorted(query.split("&")), shape


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
