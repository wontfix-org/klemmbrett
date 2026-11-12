"""JSON-like record data, and checking it against dataclasses and NamedTuples.

Record data is a tree of `None`, `bool`, `int`, `float`, `str`, lists, and
dicts with string keys; dataclass and NamedTuple instances stand for dicts.
`validate` builds such an instance from plain data, checking every field
against its annotation without converting anything, and reports the first
mismatch with a path such as `data.sender.name`.
"""

from __future__ import annotations

import dataclasses
import enum
import types
import typing
from collections.abc import Mapping, Sequence
from typing import Any

__all__ = ["SchemaError", "to_json", "validate"]


class SchemaError(TypeError):
    """Data that does not match a schema. `path` names where."""

    def __init__(self, path: str, message: str) -> None:
        super().__init__(f"{path}: {message}")
        self.path = path
        self.message = message


def _is_namedtuple(value: Any) -> bool:
    cls = value if isinstance(value, type) else type(value)
    return issubclass(cls, tuple) and hasattr(cls, "_fields")


def _is_schema(cls: Any) -> bool:
    return isinstance(cls, type) and (dataclasses.is_dataclass(cls) or _is_namedtuple(cls))


def to_json(value: Any, path: str = "data") -> Any:
    """The JSON-like form of `value`: dataclasses and NamedTuples become
    dicts, tuples become lists, and enums their values."""
    if value is None or isinstance(value, bool | int | str):
        return value
    if isinstance(value, float):
        if value != value or value in (float("inf"), float("-inf")):
            raise SchemaError(path, f"{value} is not a JSON number")
        return value
    if isinstance(value, enum.Enum):
        return to_json(value.value, path)
    if dataclasses.is_dataclass(value) and not isinstance(value, type):
        return {
            item.name: to_json(getattr(value, item.name), f"{path}.{item.name}")
            for item in dataclasses.fields(value)
        }
    if _is_namedtuple(value):
        return {name: to_json(item, f"{path}.{name}") for name, item in value._asdict().items()}
    if isinstance(value, Mapping):
        result = {}
        for key, item in value.items():
            if not isinstance(key, str):
                raise SchemaError(path, f"key {key!r} is not a string")
            result[key] = to_json(item, f"{path}.{key}")
        return result
    if isinstance(value, list | tuple):
        return [to_json(item, f"{path}[{index}]") for index, item in enumerate(value)]
    raise SchemaError(path, f"{type(value).__name__} is not JSON-like data")


def validate(value: Any, schema: Any, path: str = "data") -> Any:
    """Checks `value` against `schema` (a dataclass, a NamedTuple, or a type
    annotation) and returns it, with dicts turned into schema instances."""
    return _check(value, schema, path)


def _describe(value: Any) -> str:
    text = repr(value)
    if len(text) > 40:
        text = text[:37] + "..."
    return f"{type(value).__name__} {text}"


def _name(hint: Any) -> str:
    return getattr(hint, "__name__", None) or str(hint).replace("typing.", "")


def _check(value: Any, hint: Any, path: str) -> Any:
    if hint is Any or hint is object:
        return value
    if hint is None or hint is type(None):
        if value is not None:
            raise SchemaError(path, f"expected None, got {_describe(value)}")
        return value
    origin = typing.get_origin(hint)
    args = typing.get_args(hint)
    if origin is typing.Union or origin is types.UnionType:
        errors = []
        for option in args:
            try:
                return _check(value, option, path)
            except SchemaError as err:
                errors.append(err)
        if all(err.path == path for err in errors):
            options = " | ".join(_name(option) for option in args)
            raise SchemaError(path, f"expected {options}, got {_describe(value)}")
        raise max(errors, key=lambda err: len(err.path))
    if origin is typing.Literal:
        if not any(value == arg and type(value) is type(arg) for arg in args):
            raise SchemaError(path, f"expected one of {list(args)!r}, got {_describe(value)}")
        return value
    if origin is typing.Annotated:
        return _check(value, args[0], path)
    if origin in (list, Sequence, typing.Sequence) or hint is list:
        if not isinstance(value, list | tuple):
            raise SchemaError(path, f"expected a list, got {_describe(value)}")
        item = args[0] if args else Any
        return [_check(entry, item, f"{path}[{index}]") for index, entry in enumerate(value)]
    if origin is tuple or hint is tuple:
        if not isinstance(value, list | tuple):
            raise SchemaError(path, f"expected a list, got {_describe(value)}")
        if not args or (len(args) == 2 and args[1] is Ellipsis):
            item = args[0] if args else Any
            return tuple(_check(entry, item, f"{path}[{index}]") for index, entry in enumerate(value))
        if len(value) != len(args):
            raise SchemaError(path, f"expected {len(args)} items, got {len(value)}")
        return tuple(
            _check(entry, item, f"{path}[{index}]")
            for index, (entry, item) in enumerate(zip(value, args, strict=True))
        )
    if origin in (dict, Mapping, typing.Mapping) or hint is dict:
        if not isinstance(value, Mapping):
            raise SchemaError(path, f"expected a dict, got {_describe(value)}")
        item = args[1] if len(args) == 2 else Any
        result = {}
        for key, entry in value.items():
            if not isinstance(key, str):
                raise SchemaError(path, f"key {key!r} is not a string")
            result[key] = _check(entry, item, f"{path}.{key}")
        return result
    if _is_schema(hint):
        return _instance(value, hint, path)
    if isinstance(hint, type) and issubclass(hint, enum.Enum):
        if isinstance(value, hint):
            return value
        try:
            return hint(value)
        except ValueError:
            options = [member.value for member in hint]
            raise SchemaError(path, f"expected one of {options!r}, got {_describe(value)}") from None
    if hint is float:
        if isinstance(value, bool) or not isinstance(value, int | float):
            raise SchemaError(path, f"expected a number, got {_describe(value)}")
        return value
    if hint is int:
        if isinstance(value, bool) or not isinstance(value, int):
            raise SchemaError(path, f"expected an integer, got {_describe(value)}")
        return value
    if isinstance(hint, type):
        if not isinstance(value, hint):
            raise SchemaError(path, f"expected {hint.__name__}, got {_describe(value)}")
        return value
    raise SchemaError(path, f"unsupported annotation {hint!r}")


def _instance(value: Any, schema: type, path: str) -> Any:
    if isinstance(value, schema):
        return value
    if not isinstance(value, Mapping):
        raise SchemaError(path, f"expected {schema.__name__} fields as a dict, got {_describe(value)}")
    hints = typing.get_type_hints(schema, include_extras=True)
    if dataclasses.is_dataclass(schema):
        fields = [item for item in dataclasses.fields(schema) if item.init]
        names = [item.name for item in fields]
        required = {
            item.name
            for item in fields
            if item.default is dataclasses.MISSING and item.default_factory is dataclasses.MISSING
        }
    else:
        names = list(schema._fields)
        required = set(names) - set(schema._field_defaults)
    unknown = [key for key in value if key not in names]
    if unknown:
        raise SchemaError(
            path,
            f"unknown field{'s' if len(unknown) > 1 else ''} {', '.join(map(repr, unknown))} "
            f"for {schema.__name__}; expected {', '.join(names)}",
        )
    missing = [name for name in names if name in required and name not in value]
    if missing:
        raise SchemaError(path, f"{schema.__name__} is missing {', '.join(map(repr, missing))}")
    arguments = {
        name: _check(value[name], hints.get(name, Any), f"{path}.{name}") for name in names if name in value
    }
    return schema(**arguments)
