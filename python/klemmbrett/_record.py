from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, field, replace
from typing import Any, Union

from ._schema import to_json, validate


@dataclass(frozen=True)
class Record:
    """One row: `label` is shown, `key` identifies it, `value` is what gets used.

    `key` and `label` default to `value`. `data` is anything JSON-like, or a
    dataclass or NamedTuple: layouts reach into it with `ui.Field("a.b")`, and
    handlers get it back as it was given. Use `dataclasses.replace` to derive
    a changed record.
    """

    value: str
    label: str = field(default=None)  # type: ignore[assignment]
    key: str = field(default=None)  # type: ignore[assignment]
    data: Any = field(default=None, hash=False)

    def __post_init__(self) -> None:
        for name in ("value", "label", "key"):
            current = getattr(self, name)
            if current is None and name != "value":
                object.__setattr__(self, name, self.value)
            elif not isinstance(current, str):
                raise TypeError(f"Record.{name} must be a string, got {current!r}")

    def _wire(self) -> tuple[str, str, str, Any]:
        return (self.key, self.label, self.value, to_json(self.data, f"record {self.key!r}: data"))

    def _json(self) -> dict[str, Any]:
        """The record as Varlink carries it."""
        wire: dict[str, Any] = {"key": self.key, "label": self.label, "value": self.value}
        if self.data is not None:
            wire["data"] = to_json(self.data, f"record {self.key!r}: data")
        return wire


RecordLike = Union[Record, str, Mapping[str, Any]]

_FIELDS = ("value", "label", "key", "data")


def to_record(item: RecordLike) -> Record:
    """A `Record`, a string (the value), or a dict with a `value`, optional
    `key` and `label`, and either `data` or any other keys as data."""
    if isinstance(item, Record):
        return item
    if isinstance(item, str):
        return Record(item)
    if isinstance(item, Mapping):
        if "value" not in item:
            raise TypeError(f"record {dict(item)!r} has no value")
        extra = {name: entry for name, entry in item.items() if name not in _FIELDS}
        if extra and "data" in item:
            raise TypeError(
                f"record {item['value']!r} has both data and extra fields "
                f"({', '.join(map(str, extra))}); use one"
            )
        data = item.get("data", extra or None)
        return Record(item["value"], label=item.get("label"), key=item.get("key"), data=data)
    raise TypeError(f"expected a Record, a string, or a dict, got {item!r}")


def from_wire(item: tuple[str, str, str, Any]) -> Record:
    key, label, value, data = item
    return Record(value, label=label, key=key, data=data)


def with_schema(record: Record, schema: Any, where: str) -> Record:
    """`record` with its data checked against `schema` and turned into it."""
    data = validate(record.data, schema, f"{where}: record {record.key!r}: data")
    return record if data is record.data else replace(record, data=data)
