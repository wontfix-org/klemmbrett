from __future__ import annotations

import dataclasses
import enum
from collections.abc import Mapping
from typing import Any, Protocol, Union, runtime_checkable


@runtime_checkable
class SupportsJson(Protocol):
    """A record that encodes itself, such as `klemmbrett.Record`."""

    def _json(self) -> dict[str, Any]: ...


RecordLike = Union[str, Mapping[str, Any], SupportsJson]

_FIELDS = ("value", "label", "key", "data")


def _is_namedtuple(value: Any) -> bool:
    return isinstance(value, tuple) and hasattr(type(value), "_fields")


def _to_json(value: Any, path: str) -> Any:
    """The JSON-like form of `value`: dataclasses and NamedTuples become
    dicts, tuples become lists, and enums their values."""
    if value is None or isinstance(value, bool | int | str):
        return value
    if isinstance(value, float):
        if value != value or value in (float("inf"), float("-inf")):
            raise TypeError(f"{path}: {value} is not a JSON number")
        return value
    if isinstance(value, enum.Enum):
        return _to_json(value.value, path)
    if dataclasses.is_dataclass(value) and not isinstance(value, type):
        return {
            item.name: _to_json(getattr(value, item.name), f"{path}.{item.name}")
            for item in dataclasses.fields(value)
        }
    if _is_namedtuple(value):
        return {name: _to_json(item, f"{path}.{name}") for name, item in value._asdict().items()}
    if isinstance(value, Mapping):
        result = {}
        for key, item in value.items():
            if not isinstance(key, str):
                raise TypeError(f"{path}: key {key!r} is not a string")
            result[key] = _to_json(item, f"{path}.{key}")
        return result
    if isinstance(value, list | tuple):
        return [_to_json(item, f"{path}[{index}]") for index, item in enumerate(value)]
    raise TypeError(f"{path}: {type(value).__name__} is not JSON-like data")


def _encode_record(item: RecordLike) -> dict[str, Any]:
    """A record as Varlink carries it. A string is the value; a dict has a
    `value`, optional `key` and `label`, and either `data` or any other keys
    as data. `key` and `label` default to the value."""
    if isinstance(item, SupportsJson) and not isinstance(item, str | Mapping):
        return item._json()
    if isinstance(item, str):
        return {"key": item, "label": item, "value": item}
    if not isinstance(item, Mapping):
        raise TypeError(f"expected a Record, a string, or a dict, got {item!r}")
    if "value" not in item:
        raise TypeError(f"record {dict(item)!r} has no value")
    extra = {name: entry for name, entry in item.items() if name not in _FIELDS}
    if extra and "data" in item:
        raise TypeError(
            f"record {item['value']!r} has both data and extra fields "
            f"({', '.join(map(str, extra))}); use one"
        )
    value = item["value"]
    wire: dict[str, Any] = {
        "key": item.get("key") if item.get("key") is not None else value,
        "label": item.get("label") if item.get("label") is not None else value,
        "value": value,
    }
    for name in ("value", "label", "key"):
        if not isinstance(wire[name], str):
            raise TypeError(f"Record.{name} must be a string, got {wire[name]!r}")
    data = item.get("data", extra or None)
    if data is not None:
        wire["data"] = _to_json(data, f"record {wire['key']!r}: data")
    return wire
