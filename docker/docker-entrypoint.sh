#!/usr/bin/env bash
set -euo pipefail

pathrex_bin="${PATHREX_BIN:-/usr/local/bin/pathrex}"

if [ "${1-}" = "bench" ]; then
    shift
    has_output=false
    has_checkpoint=false
    has_criterion_dir=false
    args=("$@")

    for arg in "${args[@]}"; do
        case "$arg" in
            -o|-o*|--output|--output=*)
                has_output=true
                ;;
            -c|-c*|--checkpoint|--checkpoint=*)
                has_checkpoint=true
                ;;
            --criterion-dir|--criterion-dir=*)
                has_criterion_dir=true
                ;;
        esac
    done

    if [ "$has_output" = false ]; then
        args+=(--output /results/bench_results.json)
    fi

    if [ "$has_checkpoint" = false ]; then
        args+=(--checkpoint /results/bench_checkpoint.json)
    fi

    if [ "$has_criterion_dir" = false ]; then
        args+=(--criterion-dir /results/criterion)
    fi

    exec "$pathrex_bin" bench "${args[@]}"
fi

exec "$pathrex_bin" "$@"
