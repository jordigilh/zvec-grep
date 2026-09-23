#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
rust_dir="$(cd -- "${script_dir}/.." && pwd)"
image="${ZVEC_GREP_IMAGE:-zvec-grep-fork:codegraph-sidecar}"

usage() {
    printf '%s\n' \
        "Usage:" \
        "  $0 build" \
        "  $0 graph SOURCE_ROOT [ARTIFACT_DIR]" \
        "      [--base BASE_ARTIFACT [--changed RELATIVE_GO_PATH]... [--deleted RELATIVE_GO_PATH]...]" \
        "  $0 run SOURCE_ROOT <zg arguments...>"
}

command="${1:-}"
case "${command}" in
    build)
        docker build --file "${rust_dir}/Dockerfile" --tag "${image}" "${rust_dir}"
        ;;
    graph)
        source_root="${2:?source root is required}"
        artifact_dir="${PWD}/.zvec-grep-container"
        if [[ -n "${3:-}" && "${3}" != --* ]]; then
            artifact_dir="${3}"
            shift 3
        else
            shift 2
        fi
        source_root="$(cd -- "${source_root}" && pwd)"
        mkdir -p "${artifact_dir}"
        artifact_dir="$(cd -- "${artifact_dir}" && pwd)"
        graph_args=()
        extra_mounts=()
        while (($#)); do
            case "$1" in
                --base)
                    base_path="${2:?base artifact path is required}"
                    if [[ ! -f "${base_path}" ]]; then
                        printf 'Base artifact does not exist: %s\n' "${base_path}" >&2
                        exit 2
                    fi
                    base_path="$(cd -- "$(dirname -- "${base_path}")" && pwd)/$(basename -- "${base_path}")"
                    extra_mounts+=(--mount "type=bind,src=${base_path},dst=/base/codegraph-v1.json,readonly")
                    graph_args+=(--base /base/codegraph-v1.json)
                    shift 2
                    ;;
                --changed|--deleted)
                    graph_args+=("$1" "${2:?relative Go path is required}")
                    shift 2
                    ;;
                *)
                    printf 'Unknown graph argument: %s\n' "$1" >&2
                    usage >&2
                    exit 2
                    ;;
            esac
        done
        docker run --rm \
            --user "$(id -u):$(id -g)" \
            --mount "type=bind,src=${source_root},dst=/workspace,readonly" \
            --mount "type=bind,src=${artifact_dir},dst=/artifact" \
            "${extra_mounts[@]}" \
            "${image}" graph /workspace --output /artifact/codegraph-v1.json "${graph_args[@]}"
        ;;
    run)
        source_root="${2:?source root is required}"
        shift 2
        source_root="$(cd -- "${source_root}" && pwd)"
        docker run --rm \
            --user "$(id -u):$(id -g)" \
            --mount "type=bind,src=${source_root},dst=/workspace" \
            "${image}" "$@"
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac
