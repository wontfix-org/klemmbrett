# Themes and layouts

A **theme** sets colors, sizes, and other properties on the popup's
elements. A **layout** decides which elements appear, in what order, and
where the card sits. Both are Python objects:

```python
import klemmbrett as kb
from klemmbrett import layouts, themes, ui
from klemmbrett.style import Style, Theme, em

kb.configure(theme=themes.tokyo_night | Theme(element=Style(height=em(2.5))))

def card(ctx):
    return ui.Window(
        ui.InputBar(ui.Prompt(text="›"), ui.Entry()),
        ui.ListView(ui.Element(ui.Marker(), ui.Text("{label}"), ui.Spacer(), ui.Text("{key}", text_color="#888"))),
        location="center" if ctx.multi else "pointer",
    )

kb.widget("marks", bookmarks, layout=card)
```

## Composition

A widget's look is resolved in this order, later layers winning per
property:

1. The built-in default.
2. The palette: the widget's `theme` if set, otherwise the configured
   `theme` (or the one `kb.set_theme` switched to).
3. `overrides` from `kb.configure`, which apply to every widget and survive
   `kb.set_theme`.
4. Style keyword arguments on layout nodes, which apply to that node only.

Within a theme, `defaults` is the fallback for every element that does not
set a property itself.

Location is chosen separately from colors:

1. `location` on the widget
2. `location` on the layout's `ui.Window`
3. `location` in the theme's `window` style
4. `pointer`

## Themes

A `Theme` maps elements to `Style` objects. Both merge with `|` like dicts,
with the right side winning per property:

```python
compact = Theme(
    element=Style(height=24, padding=(2, 8)),
    element_selected=Style(background_color="#5e81ac"),
)
kb.configure(theme=themes.nord | compact)
```

Theme elements, as keyword arguments:

```
overlay
└── window
    ├── inputbar
    │   ├── prompt
    │   └── entry
    └── listview
        └── element                (element_selected, element_hover)
            ├── element_marker
            ├── element_index
            └── element_text

toast
├── toast_summary
└── toast_body
```

`element_selected` is the keyboard/current row and uses the full box style.
`element_hover` only changes `background_color` while the pointer is over a
row. `defaults` is accepted as well.

`theme.resolve()` returns every element's final style over the built-in
default, which is handy when writing a palette.

### overlay

Full-output layer behind the card. Clicking it closes the popup. Its
background defaults to transparent so the compositor shows through.

### window

The card. Its `width` is the card width. Without one, the width is derived
from `label_max`, the font size, and the chrome, then clamped to 200–520 px.
The height is rows × (element height + listview spacing) plus chrome, capped
by `max_height` (default 600). `font_size` here is the size text is drawn at
(default 14).

### listview

The scrolling row list. `spacing` is added to each row's height.

### element

One row. `height` is the row body (default 28). The default `orientation`
is horizontal.

### element_index, element_marker, element_text

The accelerator glyph (`1`–`9`, then `a`–`z`; empty while filtering), the
multi-select mark, and the label. `width` on the index and marker is their
column width (default 16), and their default text color is `#888888`. The
marker draws its `text` (default `>`) when the row is marked.

### inputbar, prompt, entry

The filter bar (default height 36, horizontal), the prompt (the widget name,
or its `text`), and the query with a block cursor.

### toast, toast_summary, toast_body

A notification toast, its summary line, and its body (see
[notifications](notifications.md)). Unless the theme styles `toast`
itself, a toast looks like the `window`: same colors, border, radius, and
font, but never its size or location. `width` is the toast width (default
360). The summary is 20 px and the body 36 px high by default, both with a
transparent background. Action buttons use the `element_selected` style.
Toasts follow the configured theme (and `kb.set_theme`), not a widget's.

## Properties

Every element accepts the same properties. Unknown names are an error.

| Property | Type | Notes |
| --- | --- | --- |
| `padding`, `margin`, `border` | 1–4 lengths | CSS order: all; vertical, horizontal; top, horizontal, bottom; top, right, bottom, left. `margin` is not drawn. |
| `border_radius` | length | Corner radius |
| `background_color`, `border_color`, `text_color` | color | Default background `#2b2b2b`, text `#e8e8e8`, border `#555555` |
| `spacing` | length | Gap between children; on `listview` added to the row height |
| `orientation` | `"horizontal"` or `"vertical"` | |
| `width`, `height`, `max_height` | length | See the elements above |
| `font_size` | length | Drawn size on `window` |
| `location` | placement name | Meaningful on `window` |
| `text` | string | The marker's mark, or a prompt's text |

Any property may be `kb.inherit` to take the parent element's resolved
value instead. The parents are `overlay` → `window` → `listview` /
`inputbar` → `element` → `element_index` / `element_text` / `element_marker`,
with `inputbar` → `prompt` / `entry`, and `element` → `element_selected` /
`element_hover`.

### Colors

- Strings: `"#2e3440"`, `"#2e344080"`, `"#fff"`, or a CSS name such as
  `"SeaGreen"` or `"transparent"`.
- `kb.rgb(r, g, b, a=1.0)`: channels from 0 to 255, alpha from 0 to 1.
- `kb.hsl(h, s, l, a=1.0)`, `kb.hwb(h, w, b, a=1.0)`, `kb.cmyk(c, m, y, k, a=1.0)`:
  hue in degrees, the rest from 0 to 1.
- `kb.transparent`.

### Lengths

Plain numbers are pixels. `kb.em(n)` is n × 14 px, `kb.ch(n)` is n × 9 px,
`kb.mm(n)` is n × 96/25.4 px, and `kb.pct(n)` is n percent of a 1920×1080
monitor: the height for `height`, `max_height`, and `font_size`, and the
width otherwise. They are plain numbers, so arithmetic works:
`width=kb.pct(10) + 20`. They do not follow the live monitor.

Anything the old HCL files did with `locals`, `env()`, `import`, or `media`
is ordinary Python now: variables, `os.environ`, imports, and `if`.

## Layouts

A layout is a `ui.Window`, or a function of a `LayoutContext` returning
one. The function runs every time the widget opens, so it can depend on
`ctx.widget`, `ctx.multi`, and `ctx.prompt` (`True` for `klemmbrett
prompt`). Shipped layouts:

| Layout | Location | Contents |
| --- | --- | --- |
| `layouts.menu` (default) | `pointer` | the list, with accelerators |
| `layouts.filter` | `center` | an input bar over the list; typing filters labels |
| `layouts.prompt` | `center` | an input bar only; Enter submits the typed text |

`layouts.rows(ctx)` is their list (marker when multi-select, index, label)
for reuse in your own layouts.

### Nodes

| Node | May contain |
| --- | --- |
| `ui.Window(*children, location=None)` | at most one `InputBar` and one `ListView`, plus primitives |
| `ui.InputBar(*children)` | `Prompt`, `Entry`, primitives |
| `ui.ListView(element)` | exactly one `Element`, the template for every row |
| `ui.Element(*children)` | `Marker`, `Index`, `Field`, primitives |
| `ui.Row(*children)`, `ui.Column(*children)` | primitives (and whatever their parent area allows) |
| `ui.Text(template)`, `ui.Spacer()` | nothing |
| `ui.Prompt()`, `ui.Entry()`, `ui.Marker()`, `ui.Index()`, `ui.Field(path, fallback="")` | nothing |

`InputBar` and `ListView` must be direct children of the window. A window
with an `InputBar` filters as you type, and accelerators are off. A window
without a `ListView` is a prompt: Enter submits the typed text as `key`,
`label`, and `value`.

`Text` templates expand `{label}`, `{key}`, `{value}`, and paths into the
record's data (`{sender.name}`, `{tags[0]}`) inside an element, and
`{widget}`, `{query}`, and `{count}` elsewhere. `{{` and `}}` are literal
braces, and names that reach nothing are kept as written. Everything
expanded in a row is shortened to `label_max`. `ui.Field(path)` shows all
values a path reaches (`tags[*]`), joined by commas, or `fallback` when it
reaches nothing. Paths are described in [widgets](widgets.md#data).

`Spacer` takes the remaining space, which pushes later children to the far
end of a row.

Every node takes style keyword arguments that override the theme for that
node (`ui.Text("{key}", text_color="#888")`, `ui.ListView(..., spacing=2)`).
Pass `children=[...]` instead of positional children if you prefer.

## Location

A 9-point grid plus `pointer`. Names may use spaces, dashes, or underscores
(`north east`, `north-east`, `north_east`).

| Name | Placement on the output that contains the pointer |
| --- | --- |
| `pointer` | at the pointer, flipping or sliding to stay on the output |
| `center` | centered |
| `north` | top, horizontally centered |
| `north east` | top right |
| `east` | right, vertically centered |
| `south east` | bottom right |
| `south` | bottom, horizontally centered |
| `south west` | bottom left |
| `west` | left, vertically centered |
| `north west` | top left |

Anchored placements inset 8 px from the output edge. A `Show` pointer
pair still picks the output. Without a pointer, the focused window's output
is used, then the primary output, then the first output, and `pointer`
falls back to `center`.
