# Widgets, transforms, and actions

A **widget** is a named list of records. Selecting a row runs that widget's
**transform** pipeline, then its **actions** in order. A widget's optional
second label is its provider (`widget "history" "clipboard-history"`).
Omit that label when every open must pass records. Action and transform
blocks still take a type (`action "notify" "exec"`, `transform "jq" "exec"`).
Widget names must be unique. Relative `db` and `path` values are resolved
relative to the configuration file.

```hcl
widget "history" "clipboard-history" {
  layout   = "filter"
  action   = "copy-to-clipboard"
}
```

`action = "name"` keeps the named action's defaults. To pass arguments or
run more than one action, use `action "name" { }` blocks (not together
with `action =` on the same widget):

```hcl
widget "links" "file" {
  path     = "links.yaml"
  format   = "yaml"
  action "copy-to-clipboard" {}
  action "notify-send" {
    title = "${label}"
  }
}
```

## Records

Every record needs a `value`. `key` and `label` default to the value when
omitted. Keys must be unique within a widget. The popup shows the label.

`${value}`, `${key}`, and `${label}` in commands, `stdin`, and widget-passed
action arguments are expanded when the row is selected.

## Widgets

| Provider | Default action | Notes |
| --- | --- | --- |
| `clipboard-history` | `copy-to-clipboard` | Optional SQLite history |
| `file` | `copy-to-clipboard` | Reloaded each time the popup opens |
| `executables` | `launch` | `PATH` entries; first basename wins |
| `wayland-windows` | `activate-window` | Open toplevels via `zwlr_foreign_toplevel_manager_v1` |
| omitted | `copy-to-clipboard` | No records of its own; each open must pass them |

`executables` and `wayland-windows` lists are long; use `layout = "filter"`.
`layout = "prompt"` is inputbar-only: Enter returns the typed text as
`key`, `label`, and `value`. Provider records are unused.

A widget with no provider label has no record source. `show` and `prompt`
must pass `--input`, or `--format` and read stdin. An empty record array
is rejected, including for `layout = "prompt"`. The tray menu omits these
widgets, because a click cannot supply records.

```hcl
widget "pick" {
  layout = "filter"
  action = "copy-to-clipboard"
}
```

```sh
printf '%s\n' one two | klemmbrett show pick --format text
```

`multi = true` lets Tab toggle the current row and move down, and
Shift-Tab toggle and move up (fzf `--multi`). Enter runs transforms and
actions for each checked row, in the order they were marked. If nothing
is checked, Enter runs the current row. Click and accelerator keys still
pick one row. `select` and `deselect` override those keys (`"tab"` and
`"shift-tab"` by default; `"btab"` is Shift-Tab). The mark is
`element-marker` in the layout (`text` defaults to `>`).

```hcl
widget "links" "file" {
  path     = "links.yaml"
  format   = "yaml"
  layout   = "filter"
  multi    = true
  select   = "tab"
  deselect = "shift-tab"
  action   = "xdg-open"
}
```

### clipboard-history

```hcl
widget "history" "clipboard-history" {
  db                   = "history.sqlite"
  history_limit        = 50
  coalesce_growing     = true
  skip_whitespace_only = true
}
```

`db` defaults to `$XDG_DATA_HOME/klemmbrett/history.sqlite`.
`history_limit` defaults to 50. Coalescing (`true` by default) replaces the
newest row when a new selection contains it, so growing a selection does
not create one row per character. Whitespace-only selections are not stored
unless `skip_whitespace_only` is `false`.

### file

`text` is one non-empty line per record. JSON and YAML are an array of
strings or objects:

```json
["plain value", {"key": "url", "label": "Project website", "value": "https://example.com"}]
```

```yaml
- plain value
- key: url
  label: Project website
  value: https://example.com
```

```hcl
widget "snippets" "file" {
  path     = "snippets.yaml"
  format   = "yaml"
  action   = "xdg-open"
}
```

`klemmbrett show` and `klemmbrett prompt` read those same formats
(`--format text|json|yaml`, from `--input` or stdin) and pass the records
to D-Bus. On the bus each record is a dict: `value` is required, and a
missing `key` or `label` defaults to the value. An empty record array
keeps the widget's own provider. A widget with no provider rejects that
empty array.

## Actions

Named `action` blocks extend a built-in (`copy-to-clipboard`, `launch`,
`exec`, or `activate-window`). Built-in names cannot be redefined.
`xdg-open` is a shipped `launch` preset (`["xdg-open", "${value}"]`) unless
the config defines that name. `notify-send` is a shipped `exec` preset
(`["notify-send", "${title}", "${body}"]`) with `title = "klemmbrett"` and
`body = "${value}"`.

`launch` and `exec` take an optional `command` (default `["${value}"]`).
`${value}`, `${key}`, `${label}`, and names declared in `args` are
substituted in `command` and `stdin`.

| Type | Process | `launch` setting | `stdin` |
| --- | --- | --- | --- |
| `copy-to-clipboard` | copies the (possibly rewritten) value | — | — |
| `launch` | detach, do not wait | yes | — |
| `exec` | wait; log a failing exit | no | optional, default empty |
| `activate-window` | focus a `wayland-windows` toplevel | — | — |

`args` gives each extra name a default. A widget may override those names
on `action "name" { }`. Widget-passed values and defaults may use
`${value}`, `${key}`, and `${label}`. Extra names on an invocation are
rejected. `args` is only valid on `launch` and `exec`.

```hcl
action "private-firefox" "launch" {
  command = ["firefox", "--private-window", "${value}"]
}

widget "links" "file" {
  path     = "links.yaml"
  format   = "yaml"
  action "copy-to-clipboard" {}
  action "notify-send" {
    title = "${label}"
    body  = "${key}: ${value}"
  }
}
```

`stdin` on `exec` is written to the command after interpolation:

```hcl
action "write" "exec" {
  command = ["wl-copy"]
  stdin   = "${value}"
}
```

## Transforms

Named `transform` blocks rewrite the selected value before the action.
The second label is the type (`exec` or `regex`). A failing transform
skips the action. A widget lists steps in order:

```hcl
transform "jq" "exec" {
  args {
    expr = ".data"
  }
  command = ["jq", "-r", "${expr}"]
}

transform "ticket" "regex" {
  pattern = "https://[^/]+/browse/(?P<ticket>[A-Z]+-\\d+)"
  replace = "${ticket}"
  on_miss = "fail"
}

widget "payloads" "file" {
  path     = "payloads.yaml"
  format   = "yaml"
  transform "jq" {
    expr = ".title"
  }
  transform "ticket" {}
  action = "copy-to-clipboard"
}
```

Omitted widget arguments keep the defaults. An empty
`transform "ticket" {}` is valid when nothing needs overriding.

### exec

Runs a command and replaces the value with stdout (one trailing newline is
stripped). `stdin` defaults to `${value}`. `${value}`, `${key}`, `${label}`,
and declared args are substituted in `command` and `stdin`. The command is
waited for and does not use `launch`.

### regex

Applies `pattern` to the current value and substitutes `replace` for every
match. `pattern` and `replace` interpolate the same names as `exec`.

`on_miss` is `keep` (default: leave the value unchanged) or `fail` (skip
the action). An invalid pattern after interpolation always fails.

Named captures use `(?P<name>…)` or `(?<name>…)` and `$name` / `${name}` in
`replace`. Capture names must not collide with `value`, `key`, `label`, or
declared args — those are expanded first. Numbered groups (`$1`, `${1}`)
work as usual. Inline flags such as `(?i)` are allowed; there is no
separate flags property.
