"""Convert the parity cases into the differential corpus, mechanically.

Each case of `tests/parity/cases/*.json` (read, never written) becomes one
row of `corpus/parity.jsonl`, in file then case order: the invocation a
user would type for the case's operation and input on the in-tree CLI.

- argv: the service, the operation and one argument per input member,
  each named as the CLI names it (botocore's `xform_name`, joined by `-`).
  A boolean member is `--name` or `--no-name`; a scalar is its text; a
  structure, list, map, union or document is compact JSON; the case
  encoding's `{"$blob": ...}` is its base64 text (the CLI's default binary
  format) and `{"$datetime": ...}` its ISO 8601 text. A member the model
  lacks (a validation case) is named the same way.
- settings: `region` is `--region`; `fips`, `dualstack`, `max_attempts`,
  `access_key`, `secret_key` and `session_token` are the matching AWS_*
  variables. `paginate`'s `MaxItems`, `PageSize` and `StartingToken` are
  `--max-items`, `--page-size` and `--starting-token`.
- replies: `responses`, or the one `response`, as they are; headers keep
  their order. A fake-model case reads its models from `tests/models`.

`--out FILE` writes the corpus; `--check FILE` fails unless FILE is what
the converter writes.
"""

from __future__ import annotations

import argparse
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


def arguments(operation: Any, params: dict[str, Any], xform_name: Any) -> list[str]:
    members = operation.input_shape.members if operation.input_shape else {}
    argv: list[str] = []
    for name, value in params.items():
        flag = xform_name(name, "-")
        member = members.get(name)
        boolean = member is not None and member.type_name == "boolean"
        if boolean and isinstance(value, bool):
            argv.append(f"--{flag}" if value else f"--no-{flag}")
        else:
            argv += [f"--{flag}", text(value)]
    return argv


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
    model = session.get_service_model(case["service"])
    operation = model.operation_model(case["operation"])
    if operation.has_streaming_output:
        raise SystemExit(f"{case['name']}: a streaming output needs an outfile argument")
    extra_argv, env = settings(case)
    argv = [case["service"], xform_name(case["operation"], "-")]
    argv += arguments(operation, case.get("input", {}), xform_name) + extra_argv
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
    lines = []
    for path in sorted(cases.glob("*.json")):
        for case in json.loads(path.read_text(encoding="utf-8")):
            where = f"tests/parity/cases/{path.name}#{case['name']}"
            lines.append(json.dumps(row(case, where, session, xform_name), ensure_ascii=False))
    return "".join(f"{line}\n" for line in lines)


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
