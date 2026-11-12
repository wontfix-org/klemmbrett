# Widgets, providers, and handlers

A **widget** is a named popup listing records from a **provider**. Picking a
row calls the widget's **`on_select`** with that `Record`. Everything that
happens next is plain Python. There are no actions, transforms, or
`${...}` placeholders.

```python
import klemmbrett as kb
from klemmbrett import layouts, providers

kb.widget("history", providers.ClipboardHistory(), layout=layouts.filter)

@kb.widget("links", providers.File("links.yaml"), layout=layouts.filter)
def links(record):
    kb.launch(["xdg-open", record.value])
```

`kb.widget(name, provider=None, *, layout, theme, location, multi, select,
deselect, on_select, on_highlight)` returns a `Widget`. Calling the `Widget`
with a function sets `on_select`, which is what makes the decorator form
work. `Widget.on_highlight` is a decorator too, and both can also be passed
as keyword arguments. Widget names must be unique. Relative paths given to
providers are resolved against the directory of `config.py`, and that
directory is importable, so helpers can live next to the config.

## Records

`kb.Record(value, label=None, key=None)` is a frozen dataclass. `key` and
`label` default to `value`. The popup shows the label, keys must be unique
within a widget, and `dataclasses.replace(record, value=...)` derives a
changed record.

Wherever records are returned or passed (providers, `kb.show`), a string
stands for `Record(string)` and a dict needs `value`, with optional `key`
and `label`.

## What picking a row does

The first of these that applies runs:

1. the widget's `on_select`
2. the provider's `select` method
3. `kb.copy(record.value)`

| Provider | Default on pick | Records |
| --- | --- | --- |
| `ClipboardHistory()` | copy | Recent clips, newest first, in SQLite |
| `File(path, format=None)` | copy | Reloaded each time the popup opens |
| `Executables()` | `kb.launch([value])` | `PATH` entries; first basename wins |
| `WaylandWindows()` | `kb.activate_window(key)` | Open toplevels via `zwlr_foreign_toplevel_manager_v1` |
| `Themes()` | `kb.set_theme(value)` | `default`, then every shipped palette |
| `Static(records)` | copy | A fixed list |
| none | copy | No records of its own; each open must pass them |

An exception in a handler is printed with its traceback (to the journal
under systemd) and ends that handler; the daemon keeps running. Handlers
run one at a time on a dedicated thread, in the order the events
happened, so a slow handler never freezes the popup.

`klemmbrett prompt` and the D-Bus `Prompt` method return the selected
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
| `kb.quit()` | Stops the daemon |

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

`klemmbrett show` and `klemmbrett prompt` read the same formats from a
positional path, and `-` reads stdin. On D-Bus each record is a dict:
`value` is required, and a missing `key` or `label` defaults to the value.
An empty record array keeps the widget's own provider, and a widget with no
provider rejects it.

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

`records()` runs on a background thread each time the popup opens, so it may
block briefly (reading files, calling a command), but slow providers delay
the popup.

## Recipes

These are the Python equivalents of the removed transforms and actions.

Copy and notify (several actions):

```python
@kb.widget("links", providers.File("links.yaml"))
def links(record):
    kb.copy(record.value)
    subprocess.run(["notify-send", record.label, record.value])
```

Open a URL, or start any detached program (`launch`, `xdg-open`):

```python
@kb.widget("bookmarks", providers.File("bookmarks.yaml"))
def bookmarks(record):
    kb.launch(["firefox", "--private-window", record.value])
```

Extract with a regular expression (`regex` with `on_miss = "fail"`):

```python
TICKET = re.compile(r"https://[^/]+/browse/(?P<ticket>[A-Z]+-\d+)")

@kb.widget("tickets", providers.ClipboardHistory())
def tickets(record):
    if match := TICKET.search(record.value):
        kb.copy(match["ticket"])
```

Pick a JSON field (`jq`):

```python
@kb.widget("payloads", providers.File("payloads.yaml"))
def payloads(record):
    kb.copy(json.loads(record.value)["data"]["title"])
```

Filter through a command (`exec` with `stdin`):

```python
@kb.widget("decode", providers.ClipboardHistory())
def decode(record):
    result = subprocess.run(["base64", "-d"], input=record.value, capture_output=True, text=True, check=True)
    kb.copy(result.stdout)
```

Paste-style pickers that print instead of acting are `klemmbrett prompt`:

```sh
printf '%s\n' one two | klemmbrett prompt pick -
```
