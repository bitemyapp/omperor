#!/bin/sh
set -eu

launcher_dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
IFS= read -r helper_path < "$launcher_dir/omp-path"
exec "$helper_path" "$@"
