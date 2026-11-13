# Widgets, providers, and handlers

A **widget** is a named popup listing records from a **provider**. Picking a
row calls the widget's **`on_select`** with that `Record`. Everything that
happens next is plain Python.

```python
import klemmbrett as kb
from klemmbrett import layouts, providers

kb.widget("history", providers.ClipboardHistory(), layout=layouts.filter)

@kb.widget("links", providers.File("links.yaml"), layout=layouts.filter)
def links(record):
    kb.launch(["xdg-open", record.value])
```

`kb.widget(name, provider=None, *, layout, theme, location, multi, select,
deselect, on_select, on_highlight, search, match, rank, prefer)` returns a
`Widget`. `match`, `rank`, and `prefer` choose how typing matches rows and
how past picks order them; see [`ranking.md`](ranking.md). Calling the `Widget`
with a function sets `on_select`, which is what makes the decorator form
work. `Widget.on_highlight` is a decorator too, and both can also be passed
as keyword arguments. Widget names must be unique. Relative paths given to
providers are resolved against the directory of `config.py`, and that
directory is importable, so helpers can live next to the config.

## Records

`kb.Record(value, label=None, key=None, data=None)` is a frozen dataclass.
`key` and `label` default to `value`. The popup shows the label, keys must
be unique within a widget, and `dataclasses.replace(record, value=...)`
derives a changed record.

Wherever records are returned or passed (providers, `kb.show`), a string
stands for `Record(string)` and a dict needs `value`, with optional `key`,
`label`, and `data`. Any other keys of a dict become its data, so
`{"value": "v", "unread": 3}` has the data `{"unread": 3}`; a dict with both
`data` and other keys is rejected.

### Data

`data` is anything JSON-like: `None`, booleans, numbers, strings, lists, and
dicts with string keys, nested as deep as needed. Dataclass and NamedTuple
instances count as dicts of their fields. Handlers get the record back with
the data exactly as the provider returned it, dataclass and all.

Layouts and filters reach into the data with paths. `sender.name` walks
dicts, `tags[0]` indexes a list, and `tags[*]` takes every element. `key`,
`label`, and `value` on their own are the record's own strings.

```python
from klemmbrett import ui

row = ui.Element(ui.Index(), ui.Text("{label}"), ui.Spacer(), ui.Field("sender.name", fallback="?"))
kb.widget("mail", mail_provider, layout=layouts.filter, search=["label", "sender.name", "tags[*]"])
```

`ui.Text("{sender.name}: {subject}")` expands paths inside a row template.
`ui.Field(path)` shows every value the path reaches, joined by commas;
strings show as they are and other values as compact JSON. `search=` sets
what typing into the filter matches; it defaults to the label. `match=`
decides how ([`ranking.md`](ranking.md)).

### Schemas

A schema checks data where it enters: give a dataclass or NamedTuple as
`schema=` to `kb.provider`, `providers.File`, or `providers.Store`. Each
record's data must then be a dict with exactly the schema's fields (or an
instance of it) and becomes an instance. Annotations are checked, and
nothing is converted: `"3"` is not an `int`, and `True` is not a number.
`list[T]`, `tuple[T, ...]`, `dict[str, T]`, `Optional`, unions, `Literal`,
enums, and nested schemas are understood. A mismatch raises
`kb.SchemaError` naming the record and the path:

```python
from typing import NamedTuple

class Bookmark(NamedTuple):
    url: str
    tags: list[str] = []

@kb.provider(schema=Bookmark)
def bookmarks():
    return [{"value": "https://x", "url": "https://x", "tags": ["docs"]}]
# a bad entry: SchemaError: <provider bookmarks>: record 'https://x': data.tags[0]: expected str, got int 3
```

## What picking a row does

The first of these that applies runs:

1. the widget's `on_select`
2. the provider's `select` method
3. `kb.copy(record.value)`

| Provider | Default on pick | Records |
| --- | --- | --- |
| `ClipboardHistory()` | copy | Recent clips, newest first, in SQLite |
| `File(path, format=None, schema=None)` | copy | Reloaded each time the popup opens |
| `Executables()` | `kb.launch([value])` | `PATH` entries; first basename wins |
| `WaylandWindows()` | `kb.activate_window(key)` | Open toplevels via `zwlr_foreign_toplevel_manager_v1`; data has `title` and `app_id` |
| `Themes()` | `kb.set_theme(value)` | `default`, then every shipped palette |
| `Static(records)` | copy | A fixed list |
| `Store(records=(), schema=None, limit=None)` | copy | Kept in memory, changed at any time |
| `provider.view(...)` | the source's | Another provider filtered, sorted, mapped |
| `Merge(*providers, prefix_keys=False)` | the source's | Several providers, one after another |
| `notifications.Stash()` | mark read | Stashed notifications, kept in SQLite ([docs](notifications.md)) |
| none | copy | No records of its own; each open must pass them |

An exception in a handler is printed with its traceback (to the journal
under systemd) and ends that handler; the daemon keeps running. Handlers
run one at a time on a dedicated thread, in the order the events
happened, so a slow handler never freezes the popup.

`kb prompt` and the Varlink `Prompt` method return the selected
values and do not call any handlers.

### Operations

These are the operations handlers can't easily do by themselves. Calling
them outside the daemon raises `RuntimeError`.

| Function | Effect |
| --- | --- |
| `kb.copy(text)` | Sets the clipboard and primary selection, and records history |
| `kb.launch(argv)` | Starts a detached program the way the `launch` setting says |
| `kb.activate_window(key)` | Focuses a `WaylandWindows` record's window |
| `kb.set_theme(theme)` | Switches the palette: a name, `"default"`, or a `Theme` |
| `kb.show(widget, records=None, pointer=None)` | Opens a widget |
| `kb.notify(summary, body="", urgency="normal", timeout=None, app_name="klemmbrett")` | Sends a notification and returns its id |
| `kb.quit()` | Stops the daemon |

`kb.notify` replaces `notify-send`. When `notifications.serve` is on, the
notification goes through your routing like any other; otherwise it goes to
the session's notification server. `urgency` is `"low"`, `"normal"`,
`"critical"`, or 0–2, and `timeout` is in milliseconds (0 never expires).

`kb.copy` goes through the sync thread, so the copied text is not copied
back as a new selection. Prefer it over `wl-copy`.

## Multi-select

`multi=True` lets Tab toggle the current row and move down, and Shift-Tab
toggle and move up (fzf `--multi`). Enter calls `on_select` once per marked
row, in the order the rows were marked. If nothing is marked, Enter picks
the current row. Clicks and accelerator keys still pick one row. `select`
and `deselect` change the keys (`"tab"` and `"shift-tab"` by default;
`"btab"` is Shift-Tab, and modifiers are written `ctrl-space`).

```python
kb.widget("links", providers.File("links.yaml"), layout=layouts.filter, multi=True)
```

The mark is drawn by `ui.Marker()` in the layout. The shipped layouts add it
when the widget is multi-select.

## Highlight

`on_highlight` runs each time the keyboard selection (arrow keys, Tab, or
typing into the filter) lands on a different row. The row the popup opens
on does not trigger it, and neither does `prompt`.

```python
palettes = kb.widget("themes", providers.Themes(), layout=layouts.filter)

@palettes.on_highlight
def preview(record):
    kb.set_theme(record.value)
```

The open popup restyles as you move through the list. Enter keeps the
current row's theme, and Escape keeps whatever was last highlighted.

## Providers

### ClipboardHistory

```python
providers.ClipboardHistory(limit=50, path=None, coalesce_growing=True, skip_whitespace_only=True)
```

`path` defaults to `$XDG_DATA_HOME/klemmbrett/history.sqlite`. Coalescing
replaces the newest row when a new selection contains it, so growing a
selection does not create one row per character. Whitespace-only selections
are not stored unless `skip_whitespace_only=False`. Every widget sharing one
`ClipboardHistory` instance shares its database, and each clip is recorded
once.

`key` is the SQLite row id. `label` and `value` are the clip text, so the
same text can appear twice and copying still uses the text.

### File

`format` is `"text"` (one non-empty line per record), `"json"`, or
`"yaml"`. When omitted, it follows the extension: `.json`, `.yaml` or
`.yml`, and anything else is text. JSON and YAML are an array of strings or
objects:

```yaml
- plain value
- key: url
  label: Project website
  value: https://example.com
```

A broken file is reported when the configuration loads, and again (in the
log) when a later edit breaks it.

Objects may carry data in a `data` field, or as any keys besides `key`,
`label`, and `value`:

```yaml
- value: https://example.com
  label: Project website
  tags: [docs, work]
```

`kb show` and `kb prompt` read the same formats from a
positional path, and `-` reads stdin. Over Varlink each record is an object
with `value`, optional `key` and `label`, and optional `data`. An empty
record list keeps the widget's own provider, and a widget with no provider
rejects it.

### Store

A `Store` keeps records in memory for handlers, threads, and timers to
change. Every change reloads open popups that show it, keeping the query,
the highlighted row, and the marks.

```python
pinned = providers.Store(limit=20)

@kb.widget("history", providers.ClipboardHistory(), layout=layouts.filter)
def history(record):
    pinned.add(record, first=True)      # replaces a record with the same key

kb.widget("pinned", pinned, on_select=lambda record: kb.copy(record.value))
```

`add(record, first=False)` replaces a record with the same key where it is,
or adds it last (or first). Past `limit`, records fall off the other end.
`extend(records)` adds several with one reload. `update(key, label=...,
value=..., data=...)`, `remove(key)`, `clear()`, `get(key)`, `len(store)`,
and iteration do what they say. `store.subscribe(callback)` calls
`callback(store)` after each change and returns a function that
unsubscribes.

### Views and merging

`provider.view(filter=None, sort=None, reverse=False, map=None, limit=None)`
shows another provider's records filtered, sorted (by a key function),
mapped, and cut to `limit`, in that order. Picking a row hands it to the
source's `select`.

```python
recent_urls = history_store.view(filter=lambda r: r.value.startswith("http"), limit=10)
```

`providers.Merge(a, b, prefix_keys=False)` lists several providers one after
another. Keys must not repeat across them, unless `prefix_keys=True` makes
them `0:key`, `1:key`, and so on. Picking a row hands the original record to
the provider it came from. Views and merges reload when a provider under
them changes.

### Changes

Any provider can call `self.changed()` when its records change; popups
showing it, directly or through views and merges, reload, and
`kb watch` prints a `changed` event. `widget.changed()` reloads one
widget. Outside the daemon both do nothing.

### Your own

A provider is a function returning records, or any object with a
`records()` method. Subclass `kb.Provider` when it needs state, a default
`select`, or the clipboard hook:

```python
@kb.provider
def bookmarks():
    with open("/home/me/bookmarks.tsv") as lines:
        return [kb.Record(url, label=title) for title, url in (line.rstrip("\n").split("\t") for line in lines)]

class Todo(kb.Provider):
    def __init__(self):
        self.items = []

    def records(self):
        return self.items

    def on_clipboard(self, text):   # every new clipboard text
        if text.startswith("TODO "):
            self.items.insert(0, text[5:])

    def select(self, record):       # the default on_select for widgets using it
        self.items.remove(record.value)

kb.widget("marks", bookmarks, on_select=lambda record: kb.launch(["xdg-open", record.value]))
kb.widget("todo", Todo())
```

`records()` runs on a background thread each time the popup opens, and again
when the provider reports a change, so it may block briefly (reading files,
calling a command), but slow providers delay the popup. `Todo` above could
call `self.changed()` after inserting, so an open popup shows the new item.

## Recipes

Copy and notify:

```python
@kb.widget("links", providers.File("links.yaml"))
def links(record):
    kb.copy(record.value)
    kb.notify(record.label, record.value)
```

Open a URL, or start any detached program:

```python
@kb.widget("bookmarks", providers.File("bookmarks.yaml"))
def bookmarks(record):
    kb.launch(["firefox", "--private-window", record.value])
```

Extract with a regular expression:

```python
TICKET = re.compile(r"https://[^/]+/browse/(?P<ticket>[A-Z]+-\d+)")

@kb.widget("tickets", providers.ClipboardHistory())
def tickets(record):
    if match := TICKET.search(record.value):
        kb.copy(match["ticket"])
```

Pick a JSON field:

```python
@kb.widget("payloads", providers.File("payloads.yaml"))
def payloads(record):
    kb.copy(json.loads(record.value)["data"]["title"])
```

Filter through a command:

```python
@kb.widget("decode", providers.ClipboardHistory())
def decode(record):
    result = subprocess.run(["base64", "-d"], input=record.value, capture_output=True, text=True, check=True)
    kb.copy(result.stdout)
```

Paste-style pickers that print instead of acting are `kb prompt`:

```sh
printf '%s\n' one two | kb prompt pick -
```
