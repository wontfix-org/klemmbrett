# Matching and ranking

Typing into a widget with an input bar filters its rows, and a widget can
order them by how they were used before. Both are set per widget:

```python
import klemmbrett as kb
from klemmbrett import layouts, providers
from klemmbrett import ranking as rk

kb.widget(
    "run",
    providers.Executables(),
    layout=layouts.filter,
    match=rk.fuzzy(),
    rank=rk.frecency(half_life="7d") + 2 * rk.learned(),
)
```

- `match=` decides which rows a query keeps and how well each one matches.
- `rank=` gives each row a score from its past use, before anything is typed.
- `prefer=` decides how the two combine.

Providers bring sensible defaults (see [below](#provider-defaults)), so
`kb.widget("run", providers.Executables(), layout=layouts.filter)` already
matches fuzzily and puts what you run often first.

Matching runs in the daemon on every keystroke and never calls Python.
Usage is kept in `$XDG_DATA_HOME/klemmbrett/usage.sqlite`.

## Matchers

All matchers look at the label, or at the fields `search=` names
(`search=["label", "sender.name"]`). A row matches when any of them does.

| Matcher | A row matches when |
| --- | --- |
| `rk.substring()` | the query occurs in it, ignoring case. The default, as before |
| `rk.prefix()` | it starts with the query, ignoring case |
| `rk.words()` | every space-separated word of the query occurs in it, in any order |
| `rk.fuzzy()` | the query's characters occur in order, with gaps |

`substring`, `prefix`, and `words` only filter: matching rows keep the
provider's order, and only the rankers reorder them.
`rk.fuzzy(scheme="default", case="smart", syntax=True)` also grades matches. It is the
[fzf](https://github.com/junegunn/fzf) algorithm (by way of
[nucleo](https://github.com/helix-editor/nucleo)): consecutive characters,
word boundaries, and camelCase humps score higher, gaps lower. Every
space-separated word must match.

- `scheme="path"` gives the bonuses to characters after `/`, for paths.
- `scheme="history"` keeps the provider's order among rows that match about
  equally well, for lists that are already newest first.
- `case="smart"` ignores case unless the query has a capital letter;
  `"ignore"` and `"respect"` always do or don't.
- With `syntax=True` (the default) a query word can be `'exact`, `^prefix`,
  `suffix$`, or `!not` (`!^`, `!$` too), as in fzf. `syntax=False` matches
  the query literally.

## Rankers

A ranker scores each row by its past use, or by anything else known when the
popup opens. Rankers add up, and a number weights them:
`rk.frecency() + 2 * rk.learned()`.

| Ranker | Scores a row by |
| --- | --- |
| `rk.frecency(half_life="3d")` | how often it was picked, each pick counting half as much after every `half_life` |
| `rk.recent()` | when it was last picked: 1 for the latest, ½ for the one before, ⅓, ... |
| `rk.count()` | how often it was picked |
| `rk.learned(prefix=3, half_life="14d")` | how often it was picked after typing what is typed now |
| `rk.pinned(keys, boost=100)` | `boost` for the keys given, or for records a function accepts |
| `rk.score(function)` | what `function(record)` returns, a number |

Durations are seconds or strings such as `"90m"`, `"12h"`, `"3d"`, `"2w"`.

`rk.learned` is what [Alfred](https://www.alfredapp.com/help/features/default-results/#knowledge)
calls knowledge: picking `Firefox` after typing `fi` makes `fi` (and `f`)
offer Firefox first next time, even when another row matches `fi` better. Only
the first `prefix` characters of a query are compared.

`rk.pinned` and `rk.score` run in Python each time the rows are read, so
they can look at anything a record carries:

```python
rank = rk.pinned(lambda r: r.data.get("starred")) + rk.score(lambda r: r.data.get("weight", 0))
```

### Namespaces

Usage is kept per widget. Rankers of several widgets can share it with
`namespace=`, so picking an application in one launcher ranks it in another:

```python
apps = rk.frecency(namespace="apps")
kb.widget("run", providers.Executables(), layout=layouts.filter, rank=apps)
kb.widget("dock", providers.File("dock.yaml"), rank=apps)
```

Picks are only recorded for widgets whose rankers use them (`frecency`,
`recent`, `count`, `learned`), under each ranker's namespace.

## Combining match and rank

| `prefer=` | With a query |
| --- | --- |
| `"blend"` (default) | the match quality (0 to 1, relative to the best match) plus half the logarithm of the rank score |
| `"match"` | the best matches first; the rank only breaks ties |
| `"usage"` | the highest rank first; the match only filters |

Without a query, rows are ordered by their rank score, and rows that score
the same keep the provider's order. Without a `rank`, the provider's order
is kept and only matching reorders.

`"blend"` lets a row you use a lot beat a slightly better match, the way most
launchers behave. `"match"` suits lists you search more than you repeat,
such as windows.

## Provider defaults

A widget that sets no `match=` or `rank=` takes them from its provider:

| Provider | `match` | `rank` |
| --- | --- | --- |
| `Executables` | `rk.fuzzy()` | `rk.frecency() + rk.learned()` |
| `WaylandWindows` | `rk.fuzzy()` | `rk.recent()` |
| `ClipboardHistory` | `rk.fuzzy(scheme="history")` | none, it is newest first already |
| `File` | `rk.fuzzy()` | none |
| others | `rk.substring()` | none |

Your own providers declare theirs as class attributes:

```python
class Bookmarks(providers.Provider):
    default_match = rk.words()
    default_rank = rk.frecency()

    def records(self):
        ...
```

`provider.view(...)` keeps its source's defaults, and `providers.Merge` keeps
them when all its providers agree. `match=None` falls back to
`rk.substring()` and `rank=None` turns ranking off.

## Usage data

`usage.sqlite` keeps, per namespace and record key, how often and when the
record was picked, the last 16 picks, and the same per typed query for
`rk.learned`. The least recently used rows go once a namespace has more than
2000. Record keys are what is stored, so a provider whose keys are the
contents (a `File` of snippets) stores those. Delete the file to start over.

## How other tools do it

The design follows what list pickers have converged on: a **matcher** that
scores how well a query fits a row, **rankers** that score rows by past use,
a **blend** of the two, and a **recorder** that notes each pick.

Matching:

- **fzf, skim, nucleo** (Helix): a fuzzy match whose score rewards word
  boundaries, camelCase transitions, and consecutive characters, and
  penalizes gaps. Ties break by length, position, or input order.
  `--scheme=path|history` tunes the bonuses for paths and for lists that are
  already chronological. The extended syntax (`'exact`, `^prefix`, `suffix$`,
  `!not`) narrows a fuzzy query. `rk.fuzzy` is this.
- **rofi** picks the matcher (`-matching normal|regex|glob|fuzzy|prefix`)
  separately from the order (`-sorting-method normal|levenshtein|fzf`), as
  `match=` and `prefer=` do here.
- **Emacs orderless** splits the query into words that all have to match, in
  any order, each in a style chosen by its first character, and
  `completion-category-overrides` choose styles per kind of data. `rk.words`
  and per-provider defaults come from there.

Usage:

- **rofi** (drun, run), **wofi**, and **fuzzel** count launches per entry and
  sort by the count before the name or the match. That is `rk.count()`.
- **zoxide** multiplies a directory's count by 4 within the hour, 2 within
  the day, ½ within the week, and ¼ after that, and ages all counts once
  their total grows. **Firefox** weighted recent visits by age buckets the
  same way, and since 2024 decays them exponentially with a half-life.
  `rk.frecency` decays each of the last 16 picks by its half-life and scales
  the sum to the full count.
- **Alfred**, **Sublime Text**, and **prescient.el** remember which item was
  picked for what was typed, so the same keys find it again. That is
  `rk.learned`.
- **VS Code** quick open and **rofi**'s window mode show the most recently
  used items first while nothing is typed, and let the match lead once
  something is. That is `rk.recent()` with `prefer="match"`.

Situation rules, which rank by the focused application, the time, or what
the caller passes in (as Raycast or Android app suggestions do), are not
built in. `rk.score` is where they go: it runs whenever the rows are read.
