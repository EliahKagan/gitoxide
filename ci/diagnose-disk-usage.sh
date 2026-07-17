#!/usr/bin/env bash

set -euo pipefail

diagnostics_dir="${GITOXIDE_DIAGNOSTICS_DIR:-${RUNNER_TEMP:-/tmp}/gitoxide-disk-diagnostics}"
sample_interval_seconds="${GITOXIDE_DIAGNOSTICS_INTERVAL_SECONDS:-45}"
top_file_limit="${GITOXIDE_DIAGNOSTICS_TOP_FILE_LIMIT:-100}"
monitor_sampler_pid=""
monitor_cleanup_done=false

function usage () {
  cat <<'EOF'
Usage:
  ci/diagnose-disk-usage.sh snapshot LABEL [--inventory]
  ci/diagnose-disk-usage.sh monitor COMMAND...
  ci/diagnose-disk-usage.sh finalize

Snapshots capture filesystem capacity, inode usage, selected directory sizes, and
pack-related files. Inventory snapshots also record machine-readable metadata for
files under the workspace, cargo directories, runner temp, and tool cache so we
can later report the largest final files, newly created files, and growth.

The monitor command keeps a periodic lightweight sampler alive while running the
requested command, preserves that command's exact exit status, always writes a
final snapshot, and generates comparison reports when inventory baselines exist.
EOF
}

function log () {
  printf '[gitoxide-disk-diagnostics] %s\n' "$*"
}

function ensure_diagnostics_dir () {
  mkdir -p "$diagnostics_dir"/{snapshots,reports,logs}
}

function snapshot_dir_for () {
  printf '%s/snapshots/%s\n' "$diagnostics_dir" "$1"
}

function append_existing_path () {
  local -n result=$1
  local path="${2:-}"

  [[ -n $path ]] || return 0
  if [[ -e $path || -L $path ]]; then
    result+=("$path")
  fi
}

function runner_work_root () {
  if [[ -n ${GITHUB_WORKSPACE:-} ]]; then
    dirname "$(dirname "$GITHUB_WORKSPACE")"
  fi
}

function inventory_roots () {
  local roots=()
  local work_root

  append_existing_path roots "${GITHUB_WORKSPACE:-}"
  append_existing_path roots "${HOME:-}/.cargo"
  append_existing_path roots "${RUNNER_TEMP:-}"

  work_root="$(runner_work_root)"
  if [[ -n $work_root ]]; then
    append_existing_path roots "${work_root}/_actions"
  fi

  printf '%s\n' "${roots[@]}"
}

function explicit_size_paths () {
  local paths=()
  local work_root

  append_existing_path paths "${GITHUB_WORKSPACE:-}"
  append_existing_path paths "${GITHUB_WORKSPACE:-}/target"
  append_existing_path paths "${GITHUB_WORKSPACE:-}/tests/fixtures/repos/linux.git"
  append_existing_path paths "${GITHUB_WORKSPACE:-}/tests/fixtures/repos/rust.git"
  append_existing_path paths "${GITHUB_WORKSPACE:-}/out"
  append_existing_path paths "${GITHUB_WORKSPACE:-}/delme"
  append_existing_path paths "${HOME:-}/.cargo"
  append_existing_path paths "${RUNNER_TEMP:-}"
  append_existing_path paths "${RUNNER_TOOL_CACHE:-}"
  append_existing_path paths "$diagnostics_dir"

  work_root="$(runner_work_root)"
  if [[ -n $work_root ]]; then
    append_existing_path paths "${work_root}/_actions"
    append_existing_path paths "${work_root}/_temp"
  fi

  printf '%s\n' "${paths[@]}"
}

function capture_best_effort () {
  local output="${1:?need output path}"
  shift
  mkdir -p "$(dirname "$output")"

  set +e
  "$@" >"$output" 2>&1
  local status=$?
  set -e
  if [[ $status -eq 0 ]]; then
    return 0
  fi

  {
    printf '\n[gitoxide-disk-diagnostics] command failed with exit code %s\n' "$status"
    printf '[gitoxide-disk-diagnostics] command:'
    printf ' %q' "$@"
    printf '\n'
  } >>"$output"
  return 0
}

function write_env_summary () {
  local output="${1:?need output path}"

  {
    printf 'timestamp=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    env | LC_ALL=C sort | grep -E '^(HOME|Image(OS|Version)|RUNNER_|ACTIONS_RUNNER_|CI=|GITHUB_(ACTOR|ACTION|ACTION_PATH|ACTION_REPOSITORY|ACTION_REF|ACTIONS|JOB|REF|REPOSITORY|RUN_ATTEMPT|RUN_ID|RUN_NUMBER|SHA|WORKFLOW|WORKSPACE)=)' || true
  } >"$output"
}

function write_explicit_sizes () {
  local output="${1:?need output path}"
  shift || true
  local path
  local data_file

  data_file="$(mktemp "${RUNNER_TEMP:-/tmp}/gitoxide-disk-diagnostics-explicit-sizes.XXXXXX")"
  {
    while IFS= read -r path; do
      if [[ -e $path || -L $path ]]; then
        du -sx -B1 -- "$path" 2>/dev/null || true
      fi
    done < <(explicit_size_paths)
  } >"$data_file"

  {
    printf 'bytes\tpath\n'
    LC_ALL=C sort -nr "$data_file"
  } >"$output"
  rm -f "$data_file"
}

function write_pack_files () {
  local output="${1:?need output path}"
  local workspace="${GITHUB_WORKSPACE:-}"

  {
    printf 'allocated_bytes\tapparent_bytes\tpath\n'
    if [[ -n $workspace && -e $workspace ]]; then
      find "$workspace" -xdev -type f \( -name '*.pack' -o -name '*.idx' -o -name 'multi-pack-index' \) \
        -printf '%b\t%s\t%p\n' 2>/dev/null | awk 'BEGIN{OFS="\t"} NR > 0 { print $1 * 512, $2, $3 }' | LC_ALL=C sort -nr || true
    fi
  } >"$output"
}

function write_inventory () {
  local output="${1:?need output path}"
  shift || true

  if [[ $# -eq 0 ]]; then
    : >"$output"
    return 0
  fi

  python3 - "$output" "$@" <<'PY'
import json
import os
import stat
import sys

output = sys.argv[1]
roots = sys.argv[2:]
diagnostics_dir = os.environ.get("GITOXIDE_DIAGNOSTICS_DIR", "")
diagnostics_dir = os.path.realpath(diagnostics_dir) if diagnostics_dir else ""


def kind_for(mode: int) -> str:
    if stat.S_ISREG(mode):
        return "file"
    if stat.S_ISLNK(mode):
        return "symlink"
    if stat.S_ISDIR(mode):
        return "dir"
    if stat.S_ISCHR(mode):
        return "char"
    if stat.S_ISBLK(mode):
        return "block"
    if stat.S_ISFIFO(mode):
        return "fifo"
    if stat.S_ISSOCK(mode):
        return "socket"
    return "other"


def record(path: str, st: os.stat_result, root: str) -> dict[str, object]:
    return {
        "root": root,
        "path": path,
        "kind": kind_for(st.st_mode),
        "mode": st.st_mode,
        "device": st.st_dev,
        "inode": st.st_ino,
        "apparent_size_bytes": st.st_size,
        "allocated_bytes": st.st_blocks * 512,
        "mtime_ns": st.st_mtime_ns,
        "ctime_ns": st.st_ctime_ns,
    }


def is_under_diagnostics(path: str) -> bool:
    if not diagnostics_dir:
        return False
    real_path = os.path.realpath(path)
    return real_path == diagnostics_dir or real_path.startswith(diagnostics_dir + os.sep)


with open(output, "w", encoding="utf-8") as out:
    for root in roots:
        if not root:
            continue
        if is_under_diagnostics(root):
            continue
        try:
            root_stat = os.lstat(root)
        except FileNotFoundError:
            continue
        except OSError as exc:
            print(f"warning: unable to stat root {root!r}: {exc}", file=sys.stderr)
            continue

        root_dev = root_stat.st_dev
        if not stat.S_ISDIR(root_stat.st_mode):
            if kind_for(root_stat.st_mode) != "dir":
                out.write(json.dumps(record(root, root_stat, root), sort_keys=True))
                out.write("\n")
            continue

        for dirpath, dirnames, filenames in os.walk(root, topdown=True, followlinks=False):
            kept_dirnames: list[str] = []
            for dirname in dirnames:
                candidate = os.path.join(dirpath, dirname)
                if is_under_diagnostics(candidate):
                    continue
                try:
                    candidate_stat = os.lstat(candidate)
                except OSError as exc:
                    print(f"warning: unable to stat directory {candidate!r}: {exc}", file=sys.stderr)
                    continue
                if not stat.S_ISDIR(candidate_stat.st_mode):
                    continue
                if candidate_stat.st_dev != root_dev:
                    continue
                kept_dirnames.append(dirname)
            dirnames[:] = kept_dirnames

            for filename in filenames:
                path = os.path.join(dirpath, filename)
                if is_under_diagnostics(path):
                    continue
                try:
                    st = os.lstat(path)
                except OSError as exc:
                    print(f"warning: unable to stat file {path!r}: {exc}", file=sys.stderr)
                    continue
                if st.st_dev != root_dev:
                    continue
                if kind_for(st.st_mode) == "dir":
                    continue
                out.write(json.dumps(record(path, st, root), sort_keys=True))
                out.write("\n")
PY
}

function write_inventory_roots () {
  local output="${1:?need output path}"

  {
    printf 'root\n'
    inventory_roots
  } >"$output"
}

function collect_snapshot () {
  local label="${1:?need label}"
  local include_inventory="${2:-false}"
  local dir
  local roots=()
  local root

  ensure_diagnostics_dir
  dir="$(snapshot_dir_for "$label")"
  mkdir -p "$dir"

  log "capturing snapshot '${label}' in ${dir}"

  printf '%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$dir/timestamp.txt"
  write_env_summary "$dir/env.txt"
  capture_best_effort "$dir/uname.txt" uname -a
  capture_best_effort "$dir/os-release.txt" cat /etc/os-release
  capture_best_effort "$dir/df-human.txt" df -h
  capture_best_effort "$dir/df-inodes.txt" df -i
  capture_best_effort "$dir/df-bytes.txt" df -B1
  capture_best_effort "$dir/mount.txt" mount
  capture_best_effort "$dir/findmnt.txt" findmnt -A -o TARGET,SOURCE,FSTYPE,OPTIONS,SIZE,USED,AVAIL,USE%,INODES,IUSED,IUSE%
  capture_best_effort "$dir/lsblk.txt" lsblk -a -b -o NAME,PATH,TYPE,SIZE,FSTYPE,MOUNTPOINTS
  capture_best_effort "$dir/diskstats.txt" cat /proc/diskstats
  write_explicit_sizes "$dir/explicit-sizes.tsv"
  write_pack_files "$dir/pack-files.tsv"

  if [[ $include_inventory == true ]]; then
    while IFS= read -r root; do
      [[ -n $root ]] || continue
      roots+=("$root")
    done < <(inventory_roots)
    write_inventory_roots "$dir/inventory-roots.txt"
    if ! write_inventory "$dir/inventory.jsonl" "${roots[@]}" 2>"$dir/inventory-errors.txt"; then
      log "inventory collection for '${label}' failed; see $dir/inventory-errors.txt"
    fi
  fi
}

function compare_inventories () {
  local base_label="${1:?need base label}"
  local final_label="${2:?need final label}"
  local report_name="${3:?need report name}"
  local base_file final_file report_dir

  base_file="$(snapshot_dir_for "$base_label")/inventory.jsonl"
  final_file="$(snapshot_dir_for "$final_label")/inventory.jsonl"
  report_dir="$diagnostics_dir/reports/$report_name"

  [[ -f $base_file && -f $final_file ]] || return 0
  mkdir -p "$report_dir"

  python3 - "$base_file" "$final_file" "$report_dir" "$top_file_limit" <<'PY'
import json
import os
import sys
from typing import Any

base_path, final_path, report_dir, limit_arg = sys.argv[1:]
limit = int(limit_arg)


def load(path: str) -> dict[str, dict[str, Any]]:
    entries: dict[str, dict[str, Any]] = {}
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if not line:
                continue
            entry = json.loads(line)
            entries[entry["path"]] = entry
    return entries


def sort_key(entry: dict[str, Any]) -> tuple[int, int, str]:
    return (
        int(entry.get("allocated_bytes", 0)),
        int(entry.get("apparent_size_bytes", 0)),
        str(entry.get("path", "")),
    )


def write_jsonl(filename: str, rows: list[dict[str, Any]]) -> None:
    with open(os.path.join(report_dir, filename), "w", encoding="utf-8") as handle:
        for row in rows[:limit]:
            handle.write(json.dumps(row, sort_keys=True))
            handle.write("\n")


base = load(base_path)
final = load(final_path)

largest_final = sorted(final.values(), key=sort_key, reverse=True)

new_files = [
    entry
    for path, entry in final.items()
    if path not in base
]
new_files.sort(key=sort_key, reverse=True)

grown_files: list[dict[str, Any]] = []
for path, entry in final.items():
    previous = base.get(path)
    if previous is None:
        continue
    allocated_growth = int(entry.get("allocated_bytes", 0)) - int(previous.get("allocated_bytes", 0))
    apparent_growth = int(entry.get("apparent_size_bytes", 0)) - int(previous.get("apparent_size_bytes", 0))
    if allocated_growth <= 0 and apparent_growth <= 0:
        continue
    grown = dict(entry)
    grown["previous_allocated_bytes"] = int(previous.get("allocated_bytes", 0))
    grown["previous_apparent_size_bytes"] = int(previous.get("apparent_size_bytes", 0))
    grown["allocated_growth_bytes"] = allocated_growth
    grown["apparent_growth_bytes"] = apparent_growth
    grown_files.append(grown)

grown_files.sort(
    key=lambda entry: (
        int(entry.get("allocated_growth_bytes", 0)),
        int(entry.get("apparent_growth_bytes", 0)),
        str(entry.get("path", "")),
    ),
    reverse=True,
)

write_jsonl("largest-final-files.jsonl", largest_final)
write_jsonl("largest-new-files.jsonl", new_files)
write_jsonl("largest-growth-files.jsonl", grown_files)

summary_lines = [
    f"baseline: {os.path.basename(os.path.dirname(base_path))}",
    f"final: {os.path.basename(os.path.dirname(final_path))}",
    f"tracked final files: {len(final)}",
    f"new files: {len(new_files)}",
    f"grown files: {len(grown_files)}",
    "",
]

def render(title: str, rows: list[dict[str, Any]], key: str) -> None:
    summary_lines.append(title)
    if not rows:
        summary_lines.append("  (none)")
        summary_lines.append("")
        return
    for row in rows[:10]:
        summary_lines.append(
            f"  {int(row.get(key, 0)):>14} allocated bytes  {int(row.get('apparent_size_bytes', 0)):>14} apparent bytes  {row.get('path', '')}"
        )
    summary_lines.append("")


render("Top final files:", largest_final, "allocated_bytes")
render("Top new files:", new_files, "allocated_bytes")

summary_lines.append("Top grown files:")
if not grown_files:
    summary_lines.append("  (none)")
else:
    for row in grown_files[:10]:
        summary_lines.append(
            f"  +{int(row.get('allocated_growth_bytes', 0)):>13} allocated bytes  +{int(row.get('apparent_growth_bytes', 0)):>13} apparent bytes  {row.get('path', '')}"
        )
summary_lines.append("")

with open(os.path.join(report_dir, "summary.txt"), "w", encoding="utf-8") as handle:
    handle.write("\n".join(summary_lines))
    handle.write("\n")
PY
}

function generate_reports () {
  local final_label="${1:?need final label}"

  compare_inventories post-checkout "$final_label" "post-checkout-to-${final_label}"
  compare_inventories post-cache "$final_label" "post-cache-to-${final_label}"
  compare_inventories pre-stress "$final_label" "pre-stress-to-${final_label}"
}

function print_concise_summary () {
  local label="${1:?need snapshot label}"
  local dir report_dir

  dir="$(snapshot_dir_for "$label")"
  log "summary for snapshot '${label}'"

  if [[ -f $dir/df-human.txt ]]; then
    printf '%s\n' '--- df -h ---'
    sed -n '1,20p' "$dir/df-human.txt"
  fi

  if [[ -f $dir/explicit-sizes.tsv ]]; then
    printf '%s\n' '--- largest tracked directories/filesystems (bytes) ---'
    sed -n '1,20p' "$dir/explicit-sizes.tsv"
  fi

  for report_dir in \
    "$diagnostics_dir/reports/pre-stress-to-${label}" \
    "$diagnostics_dir/reports/post-cache-to-${label}" \
    "$diagnostics_dir/reports/post-checkout-to-${label}"
  do
    if [[ -f $report_dir/summary.txt ]]; then
      printf '%s\n' "--- $(basename "$report_dir") ---"
      cat "$report_dir/summary.txt"
    fi
  done
}

function run_sampler () {
  local counter=0

  while true; do
    counter=$((counter + 1))
    collect_snapshot "sample-$(printf '%04d' "$counter")" false || true
    sleep "$sample_interval_seconds" || return 0
  done
}

function monitor_command () {
  [[ $# -gt 0 ]] || {
    usage >&2
    return 1
  }

  ensure_diagnostics_dir
  collect_snapshot pre-stress true

  log "starting periodic sampler every ${sample_interval_seconds}s"
  run_sampler &
  monitor_sampler_pid=$!
  local exit_status=0

  cleanup() {
    local cleanup_status=$?
    if [[ ${monitor_cleanup_done} == true ]]; then
      return "$cleanup_status"
    fi
    monitor_cleanup_done=true

    if [[ -n $monitor_sampler_pid ]] && kill -0 "$monitor_sampler_pid" 2>/dev/null; then
      kill "$monitor_sampler_pid" 2>/dev/null || true
      wait "$monitor_sampler_pid" 2>/dev/null || true
    fi
    collect_snapshot post-stress true || true
    generate_reports post-stress || true
    print_concise_summary post-stress || true
    return "$cleanup_status"
  }
  trap cleanup EXIT INT TERM

  set +e
  "$@"
  exit_status=$?
  set -e
  return "$exit_status"
}

function finalize_diagnostics () {
  ensure_diagnostics_dir
  collect_snapshot workflow-finalize true || true
  generate_reports workflow-finalize || true
  print_concise_summary workflow-finalize || true
}

function main () {
  local subcommand="${1:-}"
  shift || true

  case "$subcommand" in
    snapshot)
      [[ $# -ge 1 ]] || {
        usage >&2
        return 1
      }
      local label="${1:?need snapshot label}"
      shift
      local include_inventory=false
      if [[ ${1:-} == --inventory ]]; then
        include_inventory=true
        shift
      fi
      collect_snapshot "$label" "$include_inventory"
      ;;
    monitor)
      monitor_command "$@"
      ;;
    finalize)
      finalize_diagnostics
      ;;
    ""|-h|--help|help)
      usage
      ;;
    *)
      printf 'unknown subcommand: %s\n\n' "$subcommand" >&2
      usage >&2
      return 1
      ;;
  esac
}

main "$@"
