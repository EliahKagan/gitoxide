#!/usr/bin/env bash
#
# Run ShellCheck on the shell scripts tracked in this repository: the files
# named `*.sh` or `*.bash`, and the files whose first line is a shebang naming
# a shell, directly or through `env`. Files whose `shellcheck` Git attribute is
# unset (`-shellcheck`) are skipped. The files checked and skipped are listed
# first, and ShellCheck's findings go to standard error.

set -eu -o pipefail

shebang='^#![[:blank:]]*([^[:blank:]]*/)?(env([[:blank:]]+([-][^[:blank:]]*|[^[:blank:]]*=[^[:blank:]]*))*[[:blank:]]+)?[[:alnum:]]*sh([[:blank:]]|$)'

cd -- "$(git rev-parse --show-toplevel)"

scripts="$(mktemp)"
trap 'rm -f -- "$scripts"' EXIT

git ls-files -z -- ':(exclude,attr:-shellcheck)' | while IFS= read -r -d '' path; do
  [[ -f $path ]] || continue
  case "$path" in
  *.sh | *.bash) ;;
  *)
    line=''
    IFS= read -r -n 200 line <"$path" || true
    [[ $line =~ $shebang ]] || continue
    ;;
  esac
  printf '%s\0' "$path"
done >"$scripts"

[[ -s $scripts ]] || { echo "$0: error: found no shell scripts" >&2; exit 1; }

shellcheck --version
printf '\nChecking %d shell scripts:\n' "$(tr -cd '\0' <"$scripts" | wc -c)"
tr '\0' '\n' <"$scripts"
printf '\nSkipping files with the shellcheck attribute unset:\n'
git ls-files -- ':(attr:-shellcheck)'
printf '\n'

xargs -0 shellcheck -- <"$scripts" >&2
echo 'ShellCheck found no problems.'
