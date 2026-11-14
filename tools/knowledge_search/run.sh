#!/bin/sh
set -eu
# Umango runs this in the installed tool directory and supplies ARG_* variables.
: "${ARG_QUERY:?query is required}"
tool_dir=$(pwd -P)
scratch=$(mktemp -d "${TMPDIR:-/tmp}/rag-search.XXXXXXXX")
trap 'rm -rf "$scratch"' 0
trap 'exit 1' HUP INT TERM
cp experiment.toml "$scratch/experiment.toml"
# Tantivy needs writable metadata locks even for readers. Link immutable index files
# into the per-call scratch folder so only the lock files are created there.
for build in "$tool_dir"/indexes/*; do
    [ -d "$build/tantivy" ] || continue
    build_name=${build##*/}
    target="$scratch/indexes/$build_name"
    mkdir -p "$target/tantivy"
    ln -s "$build/build.json" "$target/build.json"
    for file in "$build"/tantivy/* "$build"/tantivy/.[!.]*; do
        [ -f "$file" ] || continue
        case "$file" in *.lock) continue ;; esac
        ln -s "$file" "$target/tantivy/${file##*/}"
    done
done
cd "$scratch"
"$tool_dir/rag-cli" search --config experiment.toml --query "$ARG_QUERY" --k "${ARG_K:-8}"
