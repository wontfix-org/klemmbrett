"""How typing matches a widget's rows, and how past picks order them::

    from klemmbrett import ranking as rk

    kb.widget("run", providers.Executables(), layout=layouts.filter,
              match=rk.fuzzy(), rank=rk.frecency(half_life="7d") + 2 * rk.learned())

Matchers decide which rows a query keeps and how well each matches. Rankers
score rows by how they were picked before (kept by the daemon in
`usage.sqlite`) or by a function of the record. Rankers add up with `+` and
are weighted with a number: `rk.frecency() + 2 * rk.learned()`.
See docs/ranking.md.
"""

from __future__ import annotations

import math
import re
import sys
import traceback
from collections.abc import Callable, Iterable
from dataclasses import dataclass
from typing import Any

from ._record import Record

__all__ = [
    "Matcher",
    "Rank",
    "count",
    "frecency",
    "fuzzy",
    "learned",
    "pinned",
    "prefix",
    "recent",
    "score",
    "substring",
    "words",
]

_SCHEMES = ("default", "path", "history")
_CASES = ("smart", "ignore", "respect")
PREFER = ("blend", "match", "usage")

_UNITS = {"s": 1, "m": 60, "h": 3600, "d": 86400, "w": 7 * 86400}
_DURATION = re.compile(r"^\s*(\d+(?:\.\d+)?)\s*([smhdw])\s*$")


def _seconds(value: float | str, what: str) -> float:
    """A duration: seconds, or a string such as `"90m"`, `"12h"`, `"3d"`, `"2w"`."""
    if isinstance(value, str):
        found = _DURATION.match(value)
        if found is None:
            raise ValueError(f"{what}: expected seconds or a duration such as '3d', got {value!r}")
        seconds = float(found.group(1)) * _UNITS[found.group(2)]
    elif isinstance(value, bool) or not isinstance(value, int | float):
        raise TypeError(f"{what}: expected seconds or a duration such as '3d', got {value!r}")
    else:
        seconds = float(value)
    if not math.isfinite(seconds) or seconds <= 0:
        raise ValueError(f"{what} must be greater than zero")
    return seconds


def _namespace(namespace: str | None) -> str | None:
    if namespace is not None and (not isinstance(namespace, str) or not namespace.strip()):
        raise ValueError("namespace must be a non-empty string")
    return namespace


@dataclass(frozen=True)
class Matcher:
    """Which rows a query keeps. Build one with `substring`, `prefix`,
    `words`, or `fuzzy`."""

    kind: str
    scheme: str = "default"
    case: str = "smart"
    syntax: bool = True

    def _wire(self) -> dict[str, Any]:
        if self.kind != "fuzzy":
            return {"kind": self.kind}
        return {"kind": "fuzzy", "scheme": self.scheme, "case": self.case, "syntax": self.syntax}

    def __repr__(self) -> str:
        if self.kind != "fuzzy":
            return f"rk.{self.kind}()"
        return f"rk.fuzzy(scheme={self.scheme!r}, case={self.case!r}, syntax={self.syntax!r})"


def substring() -> Matcher:
    """The query occurs in the text, ignoring case. Keeps the provider's order."""
    return Matcher("substring")


def prefix() -> Matcher:
    """The text starts with the query, ignoring case. Keeps the provider's order."""
    return Matcher("prefix")


def words() -> Matcher:
    """Every space-separated word of the query occurs in some text, in any order."""
    return Matcher("words")


def fuzzy(*, scheme: str = "default", case: str = "smart", syntax: bool = True) -> Matcher:
    """fzf-style fuzzy matching, best matches first.

    - `scheme`: `"default"`, `"path"` (bonuses after `/`), or `"history"`
      (rows that match about equally well keep the provider's order).
    - `case`: `"smart"` (ignore case unless the query has capitals),
      `"ignore"`, or `"respect"`.
    - `syntax`: whether `'exact`, `^prefix`, `suffix$`, and `!not` work.
    """
    if scheme not in _SCHEMES:
        raise ValueError(f"fuzzy: scheme must be one of {', '.join(_SCHEMES)}")
    if case not in _CASES:
        raise ValueError(f"fuzzy: case must be one of {', '.join(_CASES)}")
    return Matcher("fuzzy", scheme=scheme, case=case, syntax=bool(syntax))


@dataclass(frozen=True)
class _Term:
    kind: str
    weight: float = 1.0
    namespace: str | None = None
    #: Extra wire fields, as sorted (name, value) pairs.
    params: tuple[tuple[str, Any], ...] = ()
    #: For terms Python evaluates: a function of a record returning a number.
    function: Callable[[Record], Any] | None = None

    def scaled(self, factor: float) -> _Term:
        return _Term(self.kind, self.weight * factor, self.namespace, self.params, self.function)


@dataclass(frozen=True)
class Rank:
    """Rankers, added up. Build one with `frecency`, `recent`, `count`,
    `learned`, `pinned`, or `score`, and combine them with `+` and `*`."""

    terms: tuple[_Term, ...]

    def __add__(self, other: Any) -> Rank:
        if not isinstance(other, Rank):
            return NotImplemented
        return Rank(self.terms + other.terms)

    def __radd__(self, other: Any) -> Rank:
        if other == 0:
            return self
        return NotImplemented

    def __mul__(self, factor: Any) -> Rank:
        if isinstance(factor, bool) or not isinstance(factor, int | float) or not math.isfinite(factor):
            return NotImplemented
        return Rank(tuple(term.scaled(float(factor)) for term in self.terms))

    __rmul__ = __mul__

    def uses_python(self) -> bool:
        return any(term.function is not None for term in self.terms)

    def score(self, record: Record) -> float:
        """The sum of the Python-evaluated rankers for `record`."""
        total = 0.0
        for term in self.terms:
            if term.function is not None:
                value = term.function(record)
                if isinstance(value, bool):
                    value = float(value)
                if not isinstance(value, int | float) or not math.isfinite(value):
                    raise TypeError(f"rk.{term.kind} returned {value!r}, not a finite number")
                total += term.weight * float(value)
        return total

    def scores(self, records: Iterable[Record]) -> list[float]:
        """`score` for each record. A ranker that raises scores 0 for that
        record, and the first failure is printed."""
        scores = []
        reported = False
        for record in records:
            try:
                scores.append(self.score(record))
            except Exception:
                if not reported:
                    print(f"klemmbrett: ranking {self!r} failed for {record!r}:", file=sys.stderr)
                    traceback.print_exc()
                    reported = True
                scores.append(0.0)
        return scores

    def _wire(self) -> list[dict[str, Any]]:
        wire = []
        for term in self.terms:
            if term.function is not None:
                continue
            item: dict[str, Any] = {"kind": term.kind, "weight": term.weight}
            if term.namespace is not None:
                item["namespace"] = term.namespace
            item.update({name: list(value) if isinstance(value, tuple) else value for name, value in term.params})
            wire.append(item)
        if self.uses_python():
            wire.append({"kind": "python", "weight": 1.0})
        return wire

    def __repr__(self) -> str:
        parts = []
        for term in self.terms:
            text = f"rk.{term.kind}()"
            parts.append(text if term.weight == 1.0 else f"{term.weight:g} * {text}")
        return " + ".join(parts)


def frecency(*, half_life: float | str = "3d", namespace: str | None = None) -> Rank:
    """How often a row was picked, each pick counting half as much after every
    `half_life` (seconds, or a duration such as `"3d"`)."""
    seconds = _seconds(half_life, "frecency: half_life")
    return Rank((_Term("frecency", namespace=_namespace(namespace), params=(("half_life", seconds),)),))


def recent(*, namespace: str | None = None) -> Rank:
    """1 for the row picked last, 1/2 for the one before, 1/3, and so on."""
    return Rank((_Term("recent", namespace=_namespace(namespace)),))


def count(*, namespace: str | None = None) -> Rank:
    """How often a row was picked."""
    return Rank((_Term("count", namespace=_namespace(namespace)),))


def learned(*, prefix: int = 3, half_life: float | str = "14d", namespace: str | None = None) -> Rank:
    """How often a row was picked after typing what is typed now, comparing
    the first `prefix` characters. Picks count half as much after `half_life`."""
    if isinstance(prefix, bool) or not isinstance(prefix, int) or prefix < 1:
        raise ValueError("learned: prefix must be a positive integer")
    seconds = _seconds(half_life, "learned: half_life")
    return Rank(
        (
            _Term(
                "learned",
                namespace=_namespace(namespace),
                params=(("half_life", seconds), ("prefix", prefix)),
            ),
        )
    )


def pinned(keys: str | Iterable[str] | Callable[[Record], Any], *, boost: float = 100.0) -> Rank:
    """`boost` for the rows whose key is in `keys`, or for the records a
    function returns something true for."""
    if isinstance(boost, bool) or not isinstance(boost, int | float) or not math.isfinite(boost):
        raise TypeError("pinned: boost must be a number")
    if callable(keys):
        accept = keys
        return Rank((_Term("pinned", function=lambda record: boost if accept(record) else 0.0),))
    keys = [keys] if isinstance(keys, str) else list(keys)
    if not all(isinstance(key, str) for key in keys):
        raise TypeError("pinned: keys must be strings, or a function of a record")
    return Rank((_Term("pinned", params=(("boost", float(boost)), ("keys", tuple(keys)))),))


def score(function: Callable[[Record], Any]) -> Rank:
    """What `function(record)` returns, a number, each time the rows are read."""
    if not callable(function):
        raise TypeError(f"score: expected a function of a record, got {function!r}")
    return Rank((_Term("score", function=function),))


def wire(matcher: Matcher, rank: Rank | None, prefer: str) -> dict[str, Any]:
    """A widget's ranking as the daemon reads it."""
    return {
        "match": matcher._wire(),
        "rank": [] if rank is None else rank._wire(),
        "prefer": prefer,
    }
