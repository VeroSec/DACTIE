#!/usr/bin/env bash
# Rule 2 (CONTRIBUTING.md): runs the linters that CI runs, with the same pinned versions.
# Usage: scripts/lint.sh [python|rust|shell|docker|actions|go]...   (no argument: all of them)
# Needs Docker; `rust` needs cargo (the toolchain of rust-toolchain.toml).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

RUFF=ghcr.io/astral-sh/ruff:0.17.5
SHELLCHECK=koalaman/shellcheck:v0.11.0
HADOLINT=hadolint/hadolint:v2.15.1
ACTIONLINT=rhysd/actionlint:1.7.12
GO=golang:1.27-bookworm

lint_python() { docker run --rm -v "$PWD:/io:ro" -w /io "$RUFF" check --no-cache .; }

lint_rust() {
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
}

lint_shell() {
    local files
    mapfile -t files < <(git ls-files '*.sh' 'scripts/hooks/*')
    docker run --rm -v "$PWD:/mnt:ro" -w /mnt "$SHELLCHECK" -x "${files[@]}"
}

lint_docker() {
    local f status=0
    while IFS= read -r f; do
        docker run --rm -i "$HADOLINT" hadolint --no-color - < "$f" | sed "s|^-|$f|" || status=1
    done < <(git ls-files '*Dockerfile')
    return "$status"
}

lint_actions() { docker run --rm -v "$PWD:/repo:ro" -w /repo "$ACTIONLINT" -color=false; }

lint_go() {
    local mod status=0
    while IFS= read -r mod; do
        mod=$(dirname "$mod")
        echo "go: $mod"
        docker run --rm -v "$PWD/$mod:/src:ro" -w /src -e GOFLAGS=-mod=readonly \
            -e GOCACHE=/tmp/gocache -e GOPATH=/tmp/gopath "$GO" sh -c '
                unformatted=$(gofmt -l .)
                if [ -n "$unformatted" ]; then echo "not gofmt-formatted: $unformatted"; exit 1; fi
                go vet ./...' || status=1
    done < <(git ls-files '*go.mod')
    return "$status"
}

targets=("$@")
[ ${#targets[@]} -gt 0 ] || targets=(python rust shell docker actions go)
status=0
for t in "${targets[@]}"; do
    echo "== $t"
    case $t in
        python) lint_python || status=1 ;;
        rust) lint_rust || status=1 ;;
        shell) lint_shell || status=1 ;;
        docker) lint_docker || status=1 ;;
        actions) lint_actions || status=1 ;;
        go) lint_go || status=1 ;;
        *) echo "unknown target: $t" >&2; exit 2 ;;
    esac
done
exit $status
