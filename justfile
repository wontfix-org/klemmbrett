# List the recipes.
default:
    @just --list

# Build the extension into .venv and install the klemmbrett command there.
dev:
    [ -x .venv/bin/python ] || uv venv .venv
    uv pip install --python .venv/bin/python "maturin[patchelf]" pytest
    .venv/bin/maturin develop

# Run the Rust tests and the Python tests (after `just dev`).
test: dev
    cargo test --workspace
    .venv/bin/python -m pytest

# Build release wheels into target/wheels.
wheel:
    .venv/bin/maturin build --release

# Tag and build the next release candidate (omit version to be prompted).
rc *args:
    "{{justfile_directory()}}/scripts/release" --rc {{args}}

# Tag and build a Python pre-release from any branch (not pushed).
pre *args:
    "{{justfile_directory()}}/scripts/release" --pre {{args}}

# Tag, build, push and upload a release (omit version to be prompted; return accepts the guess).
release *args:
    "{{justfile_directory()}}/scripts/release" {{args}}

# Run the UI placement test on installed compositors (sway, river, qtile, cosmic, or a comma list; empty runs all).
gui-test compositor="":
    {{ if compositor == "" { "" } else { "KLEMMBRETT_COMPOSITOR=" + compositor } }} cargo test --test ui_placement -- --nocapture

# Run the UI placement test in distro containers (arch, fedora, ubuntu; none runs all). COMPOSITORS limits the compositors.
gui-matrix *distros:
    ci/ui/matrix.sh {{ distros }}
