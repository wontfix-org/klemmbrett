# Theme and widget reference

This is the catalog klemmbrett actually packs. Blocks that are not listed
here (`message`, `textbox`, `mode-switcher`, `sidebar`, `scrollbar`,
`preview`, …) are rejected.

A **theme** sets colors, sizes, and other properties on catalog elements. A
**layout** is a named packing recipe: which of those elements appear, in what
order, and where the card sits. Both are written in HCL.

## Composition

Each widget gets a `Presentation` (`theme` + `location`) built in this order:

1. The embedded default (`src/theme/default.hcl`).
2. A palette: the widget’s `theme` if set, otherwise every `theme "name" {}`
   in `config.hcl`.
3. In-file catalog blocks in `config.hcl` (`window { … }`, `element "selected" { … }`, …).
4. The named layout (`layout = "filter"`, a `layout "filter" { }` block,
   or a `.hcl` path).

Later layers overwrite earlier ones for the same property. Omitted keys keep
the default look.

`theme "nord" {}` is the file-wide default palette. `theme = "dracula"` on a
widget replaces that list for that widget only; it does not stack on top of
file-wide `theme` blocks.

Location is chosen separately from colors:

1. `location` on the widget
2. `location` on the named layout
3. `window { location = … }`
4. `pointer`

Layouts used by a widget are evaluated with `widget.name`,
`widget.provider`, and `widget.multi` in scope, so packing can depend on
the widget. `widget.provider` is `clipboard-history`, `file`,
`executables`, `wayland-windows`, or an empty string when the widget has
no provider. `widget.multi` is true when the widget sets `multi = true`.

## Layouts

A layout may be:

- a `layout "name" { … }` block in `config.hcl`
- a shipped name: `menu`, `filter`, or `prompt` (also `menu.hcl` /
  `filter.hcl` / `prompt.hcl`)
- a `.hcl` path relative to the config file, or absolute

`widget { layout = "filter" }` looks up an in-file block named `filter` first,
then a shipped layout, then a file.

The only property allowed at recipe level (outside a catalog block) is
`location`. Everything else is a catalog block:

```hcl
layout "filter" {
  location = "center"
  window { children = ["inputbar", "listview"] }
  inputbar {
    children = ["prompt", "entry"]
    padding  = 8
    spacing  = 8
  }
}
```

A standalone layout file is the same body without the `layout "name" { }`
wrapper (`layouts/filter.hcl`). Theme files must not contain `configuration`,
`widget`, `action`, `transform`, `layout`, or `theme`.

Shipped recipes:

| Name | Location | Window children |
| --- | --- | --- |
| `menu` | `pointer` | `["listview"]` |
| `filter` | `center` | `["inputbar", "listview"]` |
| `prompt` | `center` | `["inputbar"]` |

A widget with no `layout` keeps the default: list-only card at the pointer,
accelerators `1`–`9` then `a`–`z`. If `window` packs an enabled `inputbar`,
typing filters labels (case-insensitive) and accelerators are off. `prompt`
has no list: Enter submits the typed text as `key`, `label`, and `value`.

## Catalog

```
overlay
└── window
    ├── inputbar          (optional)
    │   ├── prompt
    │   └── entry
    └── listview
        └── element       (one per row; states: selected, hover)
            ├── element-marker   (optional; shown when multi-select is on)
            ├── element-index
            └── element-text
```

Only `window`, `inputbar`, and `element` accept `children`. Order and
`orientation` are honored. `enabled = false` drops an element from the packed
tree (and from card-size chrome).

| Parent | Allowed children | Default children |
| --- | --- | --- |
| `window` | `inputbar`, `listview` | `["listview"]` |
| `inputbar` | `prompt`, `entry` | `["prompt", "entry"]` |
| `element` | `element-marker`, `element-index`, `element-text` | `["element-index", "element-text"]` |
| any other | none | empty |

`element "selected"` and `element "hover"` must pack the same allowed children.
States are only valid on `element`.

### overlay

Full-output layer behind the card. Clicking it closes the popup. The default
background is `transparent()` so the compositor shows through. Box properties
are painted on this layer.

### window

The card. Its `width` is the card width; if omitted, width is derived from
`label_max`, `element-text` / `window` `font_size`, and chrome, then clamped
to 200–520 px. Height is `rows × (element height + listview spacing) + chrome`,
capped by `max_height` (default 600). `font_size` here is the size used to
draw text. `location` on `window` is the fallback placement.

### listview

The scrolling row list. `spacing` is added to each row’s height. `enabled =
false` (or omitting `listview` from `window` children) hides the list.

### element

One row. `height` is the row body (default 28). `element "selected"` is the
keyboard/current row and uses the full box style. `element "hover"` only
overrides `background_color` while the pointer is over a row. Default
`orientation` is `"horizontal"`.

### element-index

Accelerator glyph (`1`–`9`, `a`–`z`). `width` is the column width (default
16). Hidden from packing when `enabled = false`, or left empty in filter
mode. Default `text_color` is `hex("888888")`.

### element-marker

Multi-select mark (`text` when the row is checked, empty otherwise).
`text` defaults to `>`. `width` is the column width (default 16). Packed
automatically when the widget sets `multi = true`, unless `enabled =
false`. Default `text_color` is `hex("888888")`. Style it from a layout:

```hcl
element {
  children = ["element-marker", "element-index", "element-text"]
}
element-marker {
  text       = "✓"
  width      = 12
  text_color = hex("a3be8c")
}
```

### element-text

The shortened record label. `font_size` here is used only to estimate card
width when `window` has no explicit `width`; drawing still uses `window`
`font_size`.

### inputbar

Filter chrome. Its presence in `window` children (and `enabled = true`)
switches the popup to type-to-filter. `height` is the bar height (default 36).
Default `orientation` is `"horizontal"`.

### prompt

Fixed text: the widget name. Not a configurable string. In a layout
the HCL variable `widget.name` is available for expressions.

### entry

The filter query plus a block cursor (`│`). There is no `placeholder`
property.

## Properties

Every catalog block accepts the same property names. Unknown names are an
error. Shared fallbacks belong in `defaults { }`. Bindings for expressions
belong in `locals { }` and are read as `local.name`.

### enabled

Type: boolean. Default: `true`.

When `false`, the element is omitted from its parent’s packed children.

### padding, margin, border

Type: one number or a list of 1–4 numbers (CSS shorthand).

| Values | Meaning |
| --- | --- |
| `8` | all sides |
| `[0, 8]` | vertical, horizontal |
| `[1, 2, 3]` | top, horizontal, bottom |
| `[1, 2, 3, 4]` | top, right, bottom, left |

`padding` and `border` are painted. `margin` is parsed and may be
`inherit()`ed, but is not applied when drawing.

### border_radius

Type: number. Default: `0`. Corner radius of the box.

### border_color, background_color, text_color

Type: color. Defaults from `defaults` in the embedded theme: background
`hex("2b2b2b")`, text `hex("e8e8e8")`, border `hex("555555")`. `overlay`,
`element`, `element-index`, `element-text`, `element-marker`, `inputbar`,
`prompt`, and `entry` default to a transparent background unless a theme
sets one.

### background_image

Accepted so files parse. A color is used as `background_color` when that
property is unset. Other values paint as transparent. Images are not drawn.

### spacing

Type: number. Default: `0`, except `8` on `element` and `inputbar`.

Gap between packed children. On `listview` it is added to the row height.

### orientation

Type: `"horizontal"` or `"vertical"`. Default: `"vertical"`, except
`"horizontal"` on `element` and `inputbar`.

### children

Type: `["name", "name", …]`. Only `window`, `inputbar`, and `element`
(including `"selected"` / `"hover"`) may list children.

### width

Type: number.

- `window`: card width. If omitted, computed from `label_max` and chrome.
- `element-index`: accelerator column (default 16).
- `element-marker`: multi-select mark column (default 16).
- other elements: parsed, not used for layout.

### height

Type: number.

- `element`: row body (default 28).
- `inputbar`: bar height (default 36).
- `window`: parsed; card height is still computed from row count and chrome.

### max_height

Type: number. Meaningful on `window` (default 600). Caps the computed card
height.

### font_size

Type: number. Default on `window`: `14`.

`window` `font_size` is the drawn size. `element-text` `font_size` (falling
back to `window`) scales the character-width estimate used when the card
width is automatic.

### location

Type: placement name (see [Location](#location)). Meaningful on `window` and
as the recipe-level widget property.

### text

Type: string. Meaningful on `element-marker` (the mark when the row is
checked; default `>`). An empty string draws nothing. Other catalog
elements parse it and ignore it.

### inherit()

`background_color = inherit()` takes the resolved value from the parent in
this tree:

`overlay` → `window` → `listview` / `inputbar` → `element` → `element-index` /
`element-text` / `element-marker`; `inputbar` → `prompt` / `entry`;
`element` → `element "selected"` / `element "hover"`.

If the parent has no value, the element’s own value (or default) is kept.

## Values

HCL expressions are evaluated with [hcl-rs](https://crates.io/crates/hcl-rs).
Strings, numbers, bools, lists, objects, conditionals, and arithmetic are
native. Klemmbrett adds these functions:

### Lengths

Bare numbers are pixels. Helpers resolve at eval time:

| Function | Meaning |
| --- | --- |
| `px(n)` | `n` pixels |
| `em(n)` | × 14 px |
| `ch(n)` | × 9 px |
| `mm(n)` | × 96/25.4 px |
| `pct(n)` | `n` percent of 1920 |
| `vpct(n)` | `n` percent of 1080 |

These sizes do not follow the live monitor. Use HCL arithmetic instead of
`calc()`: `width = pct(10) + 20`.

### Colors

- `hex("2e3440")` — 3/4/6/8 hex digits, optional leading `#` inside the string
- `rgb(r, g, b)`, `rgba(r, g, b, a)` — channels 0–255
- `hsl(h, s, l)`, `hsla(h, s, l, a)` — `s`/`l` 0–1 or 0–100
- `hwb(h, w, b)`, `cmyk(c, m, y, k)`
- `color("SeaGreen")`, `color("transparent")`
- `transparent()`

`#2e3440` is an HCL comment, not a color.

### locals and env

```hcl
locals { accent = hex("010203") }

window {
  background_color = local.accent
  width            = pct(10) + 20
}
```

`env("NAME")` reads the process environment. `env("NAME", default)` uses the
default when unset. A default that is a color or number keeps that type.

`widget.name` and `widget.provider` are set when a layout is applied to a
widget. `monitor.width` and `monitor.height` are the 1920×1080 canvas.

## Files and media

`import "extra.hcl" {}` merges another file. A name without an extension is
tried with `.hcl`. Paths are relative to the importing file, or `~/…`.
`import "local.hcl" { optional = true }` ignores a missing file. Cycles are
rejected.

`theme "nord" {}` loads a shipped palette or a path.

```hcl
media {
  min_width = 120
  window { border = 3 }
}
```

is evaluated once at load against the same 1920×1080 canvas:

| Constraint | True when |
| --- | --- |
| `min_width` / `min_height` | canvas ≥ value |
| `max_width` / `max_height` | canvas < value |
| `min_aspect_ratio` / `max_aspect_ratio` | width/height vs value |
| `monitor_id` | value is `0` |
| `enabled` | `true`, or a true-like `env()` result |

Value-level conditions can also use HCL: `border = monitor.width >= 120 ? 3 : 1`.

## Location

9-point grid plus `pointer`. Names may be spaced or hyphenated
(`north east`, `north-east`, `northeast`).

| Name | Placement on the output that contains the pointer |
| --- | --- |
| `pointer` | at the pointer, flipping/sliding to stay on the output |
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
pair still picks the output. An empty pointer array means no pointer: the
primary output is used, and `pointer` falls back to `center`.

## defaults and locals

```hcl
defaults {
  background_color = hex("2b2b2b")
  text_color       = hex("e8e8e8")
  border_color     = hex("555555")
}

locals {
  accent = hex("5e81ac")
}
```

`defaults` is the fallback for every catalog element that does not set those
properties. `locals` is only for expressions (`local.accent`).
