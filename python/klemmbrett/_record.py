from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, field
from typing import Any, Union


@dataclass(frozen=True)
class Record:
    """One row: `label` is shown, `key` identifies it, `value` is what gets used.

    `key` and `label` default to `value`. Use `dataclasses.replace` to derive
    a changed record.
    """

    value: str
    label: str = field(default=None)  # type: ignore[assignment]
    key: str = field(default=None)  # type: ignore[assignment]

    def __post_init__(self) -> None:
        for name in ("value", "label", "key"):
            current = getattr(self, name)
            if current is None and name != "value":
                object.__setattr__(self, name, self.value)
            elif not isinstance(current, str):
                raise TypeError(f"Record.{name} must be a string, got {current!r}")

    def _wire(self) -> tuple[str, str, str]:
        return (self.key, self.label, self.value)


RecordLike = Union[Record, str, Mapping[str, Any]]


def to_record(item: RecordLike) -> Record:
    if isinstance(item, Record):
        return item
    if isinstance(item, str):
        return Record(item)
    if isinstance(item, Mapping):
        unknown = set(item) - {"value", "label", "key"}
        if unknown:
            raise TypeError(f"record has unknown fields {sorted(unknown)}; use value, label, and key")
        if "value" not in item:
            raise TypeError(f"record {dict(item)!r} has no value")
        return Record(item["value"], label=item.get("label"), key=item.get("key"))
    raise TypeError(f"expected a Record, a string, or a dict, got {item!r}")


def from_wire(item: tuple[str, str, str]) -> Record:
    key, label, value = item
    return Record(value, label=label, key=key)
