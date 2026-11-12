# UI placement tests

`tests/ui_placement.rs` opens the real popup on a real compositor and checks
where the card is painted. The test config themes the card a solid
`#ff00ff`, the output is captured over `wlr-screencopy`, and the bounding box
of that colour must match `place_at_location` for the measured card size,
within one pixel.

| Case | Widget location | Pointer sent with `Show` | Expected |
| --- | --- | --- | --- |
| `center-without-pointer` | `center` | none | centred on the output |
| `pointer` | `pointer` | (200, 150) | top-left corner at the pointer |
| `pointer-flipped` | `pointer` | 30 × 20 px from the bottom-right corner | flipped up and left, still on the output |
| `south-east` | `south east` | (200, 150) | bottom right, inset 8 px |

## Compositors

| `KLEMMBRETT_COMPOSITOR` | How it runs |
| --- | --- |
| `sway` | wlroots headless backend, [`tests/ui/sway.conf`](../tests/ui/sway.conf) |
| `river` | wlroots headless backend, [`tests/ui/river-init`](../tests/ui/river-init) |
| `qtile` | `qtile start -b wayland` on the headless backend, [`tests/ui/qtile_config.py`](../tests/ui/qtile_config.py) |
| `cosmic` | `cosmic-comp` as a fullscreen window inside headless sway; the outer output is captured |

Every compositor gets its own `XDG_RUNTIME_DIR`, XDG config, data, and cache
directories, and D-Bus session, so nothing touches the logged-in session.
The output is set to 1280×800 through `wlr-randr` when it is installed.
Otherwise the compositor keeps its default mode, and the test reads the
size from the capture.

Run one locally, or leave the variable unset to run every installed one. The
variable also takes a comma-separated list:

```sh
KLEMMBRETT_COMPOSITOR=qtile cargo test --test ui_placement -- --nocapture
just gui-test qtile        # the same; `just gui-test` runs every installed one
```

Each compositor prints `ui_placement: run`, then one `pass` or `fail` line
per case with the measured card box. A compositor that is not installed
prints `ui_placement: skip` and does not fail the test. When a case fails,
its screenshot (`screenshot.ppm`) and the compositor and klemmbrett logs are
kept in `target/ui-artifacts/<compositor>/<case>/`.

## Distro matrix

[`ci/ui/matrix.sh`](../ci/ui/matrix.sh) builds one image per distro from
`ci/ui/Containerfile.<distro>`. It then runs the test once per compositor
inside that image and prints a table:

```sh
ci/ui/matrix.sh                         # arch fedora ubuntu, every compositor
COMPOSITORS="sway cosmic" ci/ui/matrix.sh arch
CONTAINER_ENGINE=docker ci/ui/matrix.sh fedora
just gui-matrix arch                    # the same script
```

```
DISTRO   COMPOSITOR RESULT
arch     sway       pass
arch     cosmic     pass
ubuntu   qtile      skip
```

The images use pixman for compositing and Mesa's software Vulkan driver for
klemmbrett's renderer, so no GPU or `/dev/dri` is needed. Each distro gets
its own cargo home and target directory under `target/ui/<distro>/`, so
rebuilds are incremental. Per-compositor logs are in
`target/ui/<distro>/<compositor>.log`, and failed cases leave their artifacts
in `target/ui/<distro>/target/ui-artifacts/`.

`skip` means the distro does not package that compositor. The images install
what each distro ships: Arch has all four. Fedora and Ubuntu install the
optional compositors only when they are available. `fail` and `no-image`
make the script exit non-zero.
