#!/usr/bin/env bash
# Disposable stress fixtures: the same fast-import stream is hashed by Git in each format.
set -euo pipefail

# Match the Git isolation used by gix-testtools. In particular, -C alone cannot
# protect a fixture from inherited object-directory, index, or worktree overrides.
for name in ${!GIT_@}; do unset "$name"; done
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_ATTR_NOSYSTEM=1
export GIT_TERMINAL_PROMPT=0 LC_ALL=C
unset CDPATH

# Also used by Make's cached, explicitly SHA-1 remote fixtures.
if [[ ${1:-} == git ]]; then
    shift
    exec git "$@"
fi

mode=${1:?usage: stress.sh packs|commitgraph GIX [SHA1_REPOSITORY ...]}
gix=${2:?provide the gix executable}
shift 2
case "$mode" in packs|commitgraph) ;; *) exit 2 ;; esac
case "$gix" in /*) ;; *) gix="$(cd "$(dirname "$gix")" && pwd)/$(basename "$gix")" ;; esac
commits=${GIX_STRESS_COMMITS:-100000}
if [[ ! $commits =~ ^[0-9]+$ ]] || (( commits < 16 )); then
    echo 'GIX_STRESS_COMMITS must be an integer of at least 16' >&2
    exit 2
fi
work=$(mktemp -d "${TMPDIR:-/tmp}/gix-stress.XXXXXX")
trap 'rm -rf -- "$work"' EXIT
export XDG_CONFIG_HOME="$work/config"

run() {
    echo "stress ($hash): $*" >&2
    time "$gix" --verbose --object-hash "$hash" no-repo "$@"
}

# Two branches merge every 100 commits. Rotating 64 paths retains varied trees;
# similar 2 KiB blobs encourage deltas. Eight checkpoints produce multiple packs
# and ordered tips for a split graph. Marks avoid embedding format-specific IDs.
generate() {
    local repo=$1 count=$2
    git init -q --bare --object-format="$hash" "$repo"
    git -C "$repo" symbolic-ref HEAD refs/heads/main
    awk -v count="$count" '
        BEGIN {
            for (j = 0; j < 64; j++) base = base "delta-friendly stress fixture\n"
            segment = int(count / 8)
            for (i = 1; i <= count; i++) {
                blob = sprintf("revision %d\n%s", i, base)
                printf "blob\nmark :%d\ndata %d\n%s\n", 2*i-1, length(blob), blob
                branch = i % 2 ? "main" : "side"
                printf "commit refs/heads/%s\nmark :%d\n", branch, 2*i
                printf "committer Stress <stress@example.com> %d +0000\ndata 0\n", 946684800+i
                if (i > 2) printf "from :%d\n", 2*(i-2)
                if (i > 2 && i % 100 == 1) printf "merge :%d\n", 2*(i-1)
                printf "M 100644 :%d file-%02d\n\n", 2*i-1, int(i/2) % 64
                if (i % segment == 0 || i == count) {
                    printf "reset refs/tags/stress-%08d\nfrom :%d\n\n", i, 2*i
                    printf "tag annotated-%08d\nfrom :%d\n", i, 2*i
                    printf "tagger Stress <stress@example.com> %d +0000\ndata 0\n\ncheckpoint\n", 946684800+i
                }
            }
            print "done"
        }
    ' | git -C "$repo" -c fastimport.unpackLimit=0 fast-import --quiet --date-format=raw --done
    test "$(git -C "$repo" rev-list --all --count)" = "$count"
    git -C "$repo" fsck --no-dangling
}

packs() {
    local repo=$1 index pack
    local out="$repo/indexed"
    mkdir "$out"
    # Iterate instead of passing globs as one positional argument: fetched
    # repositories and fast-import checkpoints can contain several pack files.
    for index in "$repo"/objects/pack/*.idx; do
        run pack verify --statistics "$index"
        run pack verify --algorithm less-memory "$index"
        run pack verify --re-encode "$index"
        pack=${index%.idx}.pack
        run pack index create -p "$pack" "$out"
    done
    for index in "$out"/*.idx; do run pack verify "$index"; done
    run pack multi-index -i "$repo/objects/pack/multi-pack-index" create "$repo"/objects/pack/*.idx
    run pack verify "$repo/objects/pack/multi-pack-index"
}

graphs() {
    local repo=$1 commit_id
    git -C "$repo" commit-graph write --reachable
    git -C "$repo" commit-graph verify
    run commit-graph verify "$repo/objects/info/commit-graph"
    rm "$repo/objects/info/commit-graph"
    git -C "$repo" for-each-ref --sort=refname --format='%(objectname)' 'refs/tags/stress-*' > "$repo/tips"
    while read -r commit_id; do
        printf '%s\n' "$commit_id" | git -C "$repo" commit-graph write --split=no-merge --stdin-commits
    done < "$repo/tips"
    # Include both final branch tips, even when the last tag is on the side branch.
    git -C "$repo" commit-graph write --split=no-merge --reachable
    test "$(find "$repo/objects/info/commit-graphs" -name '*.graph' | wc -l)" -ge 8
    git -C "$repo" commit-graph verify
    run commit-graph verify "$repo/objects/info"
}

for hash in sha1 sha256; do
    repo="$work/$hash.git"
    generate "$repo" "$commits"
    if [[ $mode == commitgraph ]]; then
        graphs "$repo"
    else
        packs "$repo"
        for index in "$repo"/objects/pack/*.idx; do run pack explode "$index"; done
        # A second encoding exercises REF_DELTA object IDs as well as the
        # OFS_DELTA offsets produced by fast-import. Verify and reindex it too.
        mkdir "$repo/ref-delta"
        git -C "$repo" pack-objects --all --no-delta-base-offset "$repo/ref-delta/pack"
        mkdir "$repo/ref-delta/indexed"
        for index in "$repo"/ref-delta/*.idx; do
            run pack verify --re-encode "$index"
            run pack verify --algorithm less-memory "$index"
            run pack index create -p "${index%.idx}.pack" "$repo/ref-delta/indexed"
            run pack explode "$index"
        done
        for index in "$repo"/ref-delta/indexed/*.idx; do run pack verify "$index"; done
        # Limit loose-file creation, while the sink above processes every object.
        small="$work/$hash-small.git"
        generate "$small" 512
        git -C "$small" repack -ad
        mkdir "$small/loose"
        for index in "$small"/objects/pack/*.idx; do
            run pack explode --verify "$index" "$small/loose"
        done
    fi
done

# Keep the real-world SHA-1 pack workloads. Copy indices and link immutable packs
# into disposable directories so multi-index creation never writes into the cache.
hash=sha1
for source in "$@"; do
    source=$(cd "$source" && pwd)
    test "$(git -C "$source" rev-parse --show-object-format)" = sha1
    repo="$work/real-$(basename "$source")"
    mkdir -p "$repo/objects/pack"
    for index in "$source"/objects/pack/*.idx; do
        cp "$index" "$repo/objects/pack/"
        ln -s "${index%.idx}.pack" "$repo/objects/pack/"
    done
    packs "$repo"
done
