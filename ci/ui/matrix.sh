#!/bin/sh
# Runs tests/ui_placement.rs for every distro x compositor pair in containers.
#
#   ci/ui/matrix.sh [distro ...]          default: arch fedora ubuntu
#   COMPOSITORS="sway qtile" ci/ui/matrix.sh arch
#   CONTAINER_ENGINE=docker ci/ui/matrix.sh
#
# Each distro gets its own cargo home and target dir under target/ui/<distro>/,
# so builds are cached between runs. Logs land next to them as
# <compositor>.log, and failed cases leave screenshots and compositor logs in
# target/ui/<distro>/target/ui-artifacts/<compositor>/<case>/.
#
# Exits non-zero if any pair failed or an image did not build. A pair whose
# compositor the distro does not package is reported as "skip".
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
engine=${CONTAINER_ENGINE:-podman}
compositors=${COMPOSITORS:-sway river qtile cosmic}
[ "$#" -gt 0 ] || set -- arch fedora ubuntu

user_args="--user $(id -u):$(id -g)"
if [ "$engine" = podman ]; then
    user_args="--userns=keep-id $user_args"
fi

table=$(mktemp)
trap 'rm -f "$table"' EXIT
status=0

for distro in "$@"; do
    image="klemmbrett-ui-$distro"
    work="$root/target/ui/$distro"
    mkdir -p "$work/cargo" "$work/target"

    echo "==> building $image" >&2
    if ! "$engine" build -t "$image" -f "$root/ci/ui/Containerfile.$distro" "$root/ci/ui" >"$work/build.log" 2>&1; then
        echo "    image build failed, see $work/build.log" >&2
        for compositor in $compositors; do
            printf '%s\t%s\t%s\n' "$distro" "$compositor" "no-image" >>"$table"
        done
        status=1
        continue
    fi

    for compositor in $compositors; do
        log="$work/$compositor.log"
        echo "==> $distro / $compositor" >&2
        # shellcheck disable=SC2086
        if "$engine" run --rm $user_args \
            -v "$root:/src:Z" -w /src \
            -e HOME=/tmp \
            -e CARGO_HOME="/src/target/ui/$distro/cargo" \
            -e CARGO_TARGET_DIR="/src/target/ui/$distro/target" \
            -e KLEMMBRETT_COMPOSITOR="$compositor" \
            "$image" \
            cargo test --locked --test ui_placement -- --nocapture >"$log" 2>&1
        then
            if grep -q "^ui_placement: skip" "$log"; then
                result=skip
            else
                result=pass
            fi
        else
            result=fail
            status=1
        fi
        printf '%s\t%s\t%s\n' "$distro" "$compositor" "$result" >>"$table"
    done
done

echo
printf '%-8s %-10s %s\n' DISTRO COMPOSITOR RESULT
while IFS="$(printf '\t')" read -r distro compositor result; do
    printf '%-8s %-10s %s\n' "$distro" "$compositor" "$result"
done <"$table"
exit "$status"
