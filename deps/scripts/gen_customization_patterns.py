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
  session registers (`Session._register_builtin_handlers`);
- the handlers a client registers on its own emitter as it is created
  (`ClientCreator.create_client`, `BaseClient._register_handlers`), which
  are code, not data: this script creates every service's client and
  records what its emitter holds beyond the session's.

This script imports those modules and writes one JSON entry,
`native/customization-patterns.json`, under `--output-root`, which the
packer stores in the payload. Nothing derived from it is committed. The
document is:

    {
      "plugin_registry": [[pattern, [[module, function], ...]], ...],
      "main_command_table_ops": [["rename", old, new] |
                                 ["add", name, module, class], ...],
      "builtin_handlers": [[pattern, handler, "first" | "last" |
                            "default"], ...],
      "client_handlers": {
        "generic": [[pattern, handler, section], ...],
        "services": [[service, [[pattern, handler, section], ...]], ...]
      },
      "command_table_edits": [
        [[module, function],
         [[event, handler, [["set" | "del", name], ...]], ...]], ...
      ]
    }

in the tables' own order. A handler is named `module.qualname`; a bound
method by its class and method name, and a callable object by its class.

`command_table_edits` holds the registry initializers whose handlers do
nothing but edit command tables (`TABLE_EDITORS`): the handlers each
registers, in its order, and what each does to the table of its event,
which is code, not data. This script runs each initializer against a
registry that records what it registers, then runs each handler against
a table that records the names it sets (`command_table[name] = ...`) and
deletes (`del command_table[name]`), in its order. The pack refuses an
initializer that registers anywhere else than a root command's own table
event, or is listed under any other pattern, and a handler that reads its
table, so what is recorded is all each does.

A client row's pattern spells the client's own service segment (its
hyphenized service id, the second chunk) as `{service_id}`. The rows
every service's client registers are `generic`; the rest are listed under
the service's loader name. Clients are created as the CLI creates one by
default (`CLIOperationCaller.invoke`: no endpoint URL; `standard`
retries; endpoint discovery `auto`; no client-side monitoring), with
static credentials and a fixed region, from botocore's own data alone,
and an audit hook refuses any connection, lookup or send.

The `--python-path` directories hold the `awscli` and `awscrt` packages.
Importing botocore's handlers imports the CRT bindings, whose extension
the packing interpreter may not load; the autocomplete generator's
stand-ins (`gen_ac_index.install_stand_ins`) take its place, as nothing
here calls into it.
"""

import argparse
import functools
import importlib
import inspect
import json
import os
import sys
from collections.abc import MutableMapping
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


SERVICE_ID = "{service_id}"
# The audit events of a socket reaching the network. urllib3 binds a
# loopback socket at import to probe for IPv6 support, which sends nothing.
NETWORK_EVENTS = {
    "socket.connect",
    "socket.getaddrinfo",
    "socket.gethostbyname",
    "socket.gethostbyaddr",
    "socket.sendto",
    "socket.sendmsg",
}


def deny_network(event: str, args: tuple[object, ...]) -> None:
    if event in NETWORK_EVENTS:
        raise RuntimeError(f"client creation attempted network access: {event}")


def emitter_rows(emitter: Any) -> list[tuple[str, str, Any]]:
    """Every `(pattern, section, handler)` a `HierarchicalEmitter` holds,
    from its prefix trie (`PrefixTrie._root`: chunk, children, values)."""
    rows = []
    pending = [("", emitter._handlers._root)]
    while pending:
        prefix, node = pending.pop()
        if node["values"] is not None:
            for section, handlers in zip(("first", "default", "last"), node["values"]):
                rows.extend((prefix, section, handler) for handler in handlers)
        for chunk, child in node["children"].items():
            pending.append((f"{prefix}.{chunk}" if prefix else chunk, child))
    return rows


def isolated_session() -> Any:
    """A botocore session reading no configuration, credentials or model
    but botocore's own data."""
    for name in [name for name in os.environ if name.startswith("AWS_")]:
        del os.environ[name]
    os.environ["AWS_CONFIG_FILE"] = os.devnull
    os.environ["AWS_SHARED_CREDENTIALS_FILE"] = os.devnull
    import botocore.loaders
    import botocore.session

    session = botocore.session.Session()
    session.register_component(
        "data_loader",
        botocore.loaders.Loader(
            extra_search_paths=[botocore.loaders.Loader.BUILTIN_DATA_PATH],
            include_default_search_paths=False,
        ),
    )
    return session


def client_rows(session: Any, service: str, base: set[Any]) -> set[tuple[str, str, str]]:
    """The rows `service`'s client registers beyond the session's `base`."""
    client = session.create_client(
        service,
        region_name="us-east-1",
        aws_access_key_id="AKIDEXAMPLE",
        aws_secret_access_key="secret",
    )
    service_id = client.meta.service_model.service_id.hyphenize()
    rows = set()
    for pattern, section, handler in emitter_rows(client.meta.events):
        if (pattern, section, id(handler)) in base:
            continue
        chunks = pattern.split(".")
        if len(chunks) > 1 and chunks[1] == service_id:
            chunks[1] = SERVICE_ID
        rows.add((".".join(chunks), handler_name(handler), section))
    return rows


def client_handlers() -> dict[str, Any]:
    session = isolated_session()
    base = {
        (pattern, section, id(handler))
        for pattern, section, handler in emitter_rows(session.get_component("event_emitter"))
    }
    services = session.get_available_services()
    sys.addaudithook(deny_network)
    observed = {service: client_rows(session, service, base) for service in services}
    generic = set.intersection(*observed.values()) if observed else set()
    return {
        "generic": [list(row) for row in sorted(generic)],
        "services": [
            [service, [list(row) for row in sorted(rows - generic)]]
            for service, rows in observed.items()
            if rows - generic
        ],
    }


# The initializers whose handlers only edit a root command's table:
# `register_removals` deletes deprecated or unsupported operations
# (`CommandRemover`), and `register_wizard_commands` sets `wizard` on each
# command with wizards (`_register_wizards_for_commands`) and `wizard-dev`
# on `cli-dev` (`wizard.devcommands.register_dev_commands`).
TABLE_EDITORS = [
    ("awscli.customizations.removals", "register_removals"),
    ("awscli.customizations.wizard.commands", "register_wizard_commands"),
]
TABLE_EVENT = "building-command-table"


def root_table_event(event: str) -> bool:
    """Whether `event` is a root command's own table event: one command
    (`ServiceCommand._create_command_table`), not the main table's and not
    a subcommand's, whose lineage is `_`-joined
    (`ServiceOperation._build_subcommand_table`)."""
    parts = event.split(".")
    return (
        len(parts) == 2
        and parts[0] == TABLE_EVENT
        and parts[1] not in ("", "main", "*")
        and "_" not in parts[1]
    )


class RecordingRegistry:
    """The emitter an initializer is given (`LazyInitEmitter` calls it with
    itself), keeping each `(event, handler)` it registers, in order."""

    def __init__(self) -> None:
        self.registered: list[tuple[str, Any]] = []

    def register(self, event_name: str, handler: Any, **kwargs: Any) -> None:
        if kwargs:
            raise SystemExit(f"unexpected registration options: {kwargs!r}")
        self.registered.append((event_name, handler))

    def __getattr__(self, name: str) -> Any:
        raise SystemExit(f"a table editor uses the emitter's {name}")


class RecordingTable(MutableMapping):
    """A command table that keeps each name a handler sets or deletes, in
    order, and refuses to be read: a handler's edits are then all it does
    to the table, whatever the table holds."""

    def __init__(self) -> None:
        self.edits: list[list[str]] = []

    def __setitem__(self, name: str, value: Any) -> None:
        self.edits.append(["set", name])

    def __delitem__(self, name: str) -> None:
        self.edits.append(["del", name])

    def __getitem__(self, name: str) -> Any:
        raise SystemExit(f"a table editor reads its table: {name!r}")

    def __iter__(self) -> Any:
        raise SystemExit("a table editor reads its table")

    def __len__(self) -> int:
        raise SystemExit("a table editor reads its table")


class StandIn:
    """The command whose table is built (`command_object`), by name."""

    def __init__(self, name: str) -> None:
        self.name = name


def command_table_edits(registry: Any) -> list[Any]:
    """Each of `TABLE_EDITORS`: the handlers it registers and the names
    each sets and deletes in its event's table, run as
    `ServiceCommand._create_command_table` emits the event (clidriver.py),
    with no session."""
    rows = []
    for module, function in TABLE_EDITORS:
        patterns = [
            pattern
            for pattern, entries in registry.PLUGIN_REGISTRY.items()
            if (module, function) in [tuple(entry) for entry in entries]
        ]
        if not patterns or not all(root_table_event(pattern) for pattern in patterns):
            raise SystemExit(f"{module}.{function} is listed under {patterns!r}")
        recorder = RecordingRegistry()
        getattr(importlib.import_module(module), function)(recorder)
        handlers = []
        for event, handler in recorder.registered:
            if not root_table_event(event):
                raise SystemExit(f"{module}.{function} registers on {event}")
            table = RecordingTable()
            handler(
                command_table=table,
                session=None,
                command_object=StandIn(event.split(".")[1]),
            )
            handlers.append([event, handler_name(handler), table.edits])
        rows.append([[module, function], handlers])
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
        "client_handlers": client_handlers(),
        "command_table_edits": command_table_edits(registry),
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
