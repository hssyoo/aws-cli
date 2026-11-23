#!/usr/bin/env python3
"""Write the customization patterns the native admission matches.

The native route decides whether a Python customization touches an
invocation by matching the events the invocation would emit against the
event patterns the AWS CLI registers. Those patterns live in Python, as
data, in the same source the fallback runs:

- `awscli.handlers_registry.PLUGIN_REGISTRY`: the lazy emitter's
  initializers, by event pattern (`LazyInitEmitter.load_registry`);
- `awscli.handlers_registry.MAIN_COMMAND_TABLE_OPS`: the renames and
  additions the lazy emitter applies to the main command table
  (`LazyInitEmitter._apply_main_command_table_ops`);
- `botocore.handlers.BUILTIN_HANDLERS`: the handlers every botocore
  session registers (`Session._register_builtin_handlers`).

This script imports those modules and writes one JSON entry,
`native/customization-patterns.json`, under `--output-root`, which the
packer stores in the payload. Nothing derived from it is committed. The
document is:

    {
      "plugin_registry": [[pattern, [[module, function], ...]], ...],
      "main_command_table_ops": [["rename", old, new] |
                                 ["add", name, module, class], ...],
      "builtin_handlers": [[pattern, handler, "first" | "last" |
                            "default"], ...]
    }

in the tables' own order. A handler is named `module.qualname`; a bound
method by its class and method name, and a callable object by its class.

The `--python-path` directories hold the `awscli` and `awscrt` packages.
Importing botocore's handlers imports the CRT bindings, whose extension
the packing interpreter may not load; the autocomplete generator's
stand-ins (`gen_ac_index.install_stand_ins`) take its place, as nothing
here calls into it.
"""

import argparse
import functools
import inspect
import json
import sys
from pathlib import Path
from typing import Any

# This script runs by path, so its own directory, which holds the
# autocomplete generator, is on the import path.
from gen_ac_index import install_stand_ins

ENTRY = Path("native") / "customization-patterns.json"


def handler_name(handler: Any) -> str:
    """The dotted name a botocore handler is identified by."""
    target = handler
    while isinstance(target, functools.partial):
        target = target.func
    if inspect.ismethod(target):
        owner = target.__self__
        cls = owner if inspect.isclass(owner) else type(owner)
        return f"{cls.__module__}.{cls.__qualname__}.{target.__name__}"
    if inspect.isfunction(target) or inspect.isbuiltin(target):
        return f"{target.__module__}.{target.__qualname__}"
    cls = type(target)
    return f"{cls.__module__}.{cls.__qualname__}"


def builtin_rows(handlers: Any) -> list[list[str]]:
    """`BUILTIN_HANDLERS` as `[pattern, handler, register type]` rows."""
    sections = {
        id(handlers.REGISTER_FIRST): "first",
        id(handlers.REGISTER_LAST): "last",
    }
    rows = []
    for row in handlers.BUILTIN_HANDLERS:
        if len(row) not in (2, 3):
            raise SystemExit(f"unexpected BUILTIN_HANDLERS row: {row!r}")
        section = sections.get(id(row[2]), None) if len(row) == 3 else "default"
        if section is None:
            raise SystemExit(f"unknown register type in row: {row!r}")
        rows.append([row[0], handler_name(row[1]), section])
    return rows


def table_ops(registry: Any) -> list[list[str]]:
    """`MAIN_COMMAND_TABLE_OPS` with each op as its string value."""
    rows = []
    for op in registry.MAIN_COMMAND_TABLE_OPS:
        kind = registry.CommandTableOp(op[0]).value
        expected = {"rename": 3, "add": 4}.get(kind)
        if expected != len(op):
            raise SystemExit(f"unexpected MAIN_COMMAND_TABLE_OPS row: {op!r}")
        rows.append([kind, *op[1:]])
    return rows


def patterns() -> dict[str, Any]:
    # `awscli` puts its vendored botocore on the import path as `botocore`.
    import awscli  # noqa: F401
    import awscli.handlers_registry as registry
    import botocore.handlers as handlers

    return {
        "plugin_registry": [
            [pattern, [list(entry) for entry in entries]]
            for pattern, entries in registry.PLUGIN_REGISTRY.items()
        ],
        "main_command_table_ops": table_ops(registry),
        "builtin_handlers": builtin_rows(handlers),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-root", type=Path, required=True)
    parser.add_argument("--python-path", action="append", default=[])
    args = parser.parse_args()
    sys.dont_write_bytecode = True
    sys.path[:0] = args.python_path
    install_stand_ins()
    entry = args.output_root / ENTRY
    entry.parent.mkdir(parents=True, exist_ok=True)
    text = json.dumps(patterns(), separators=(",", ":"), ensure_ascii=False)
    entry.write_text(text + "\n", encoding="utf-8")
    print(f"wrote {entry}", file=sys.stderr)


if __name__ == "__main__":
    main()
