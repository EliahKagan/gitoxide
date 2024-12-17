# Must be sourced into the main journey test

function set-static-git-environment() {
  set -a
  export GIT_AUTHOR_DATE="2020-09-09 09:06:03 +0800"
  export GIT_COMMITTER_DATE="${GIT_AUTHOR_DATE}"
  export GIT_AUTHOR_NAME="Sebastian Thiel"
  export GIT_COMMITTER_NAME="${GIT_AUTHOR_NAME}"
  export GIT_AUTHOR_EMAIL="git@example.com"
  export GIT_COMMITTER_EMAIL="${GIT_AUTHOR_EMAIL}"
  set +a
}

function remove-paths() {
  sed -E 's#/.*#"#g'
}

function repo-with-remotes() {
  if [[ $((($# - 1) % 2)) != 0 ]] || [[ $# = 0 ]]; then
    echo "need <path> (<remote> <url>)[,...] tuples"
    exit 42
  fi

  mkdir -p "$1"
  (
    cd "$1"
    shift
    git init
    while [[ $# != 0 ]]; do
        git remote add "$1" "$2"
        shift 2
    done
    git config commit.gpgsign false
    git config tag.gpgsign false
    touch a
    git add a
    git commit -m "non-bare"
  ) &>/dev/null
}

function small-repo-in-sandbox() {
  sandbox
  {
    git init
    git checkout -b main
    git config commit.gpgsign false
    git config tag.gpgsign false
    touch a
    git add a
    git commit -m "first"
    git tag unannotated
    touch b
    git add b
    git commit -m "second"
    git tag annotated -m "tag message"
    git branch dev
    echo hi >> b
    git commit -am "third"
  } &>/dev/null
}

function launch-git-daemon() {
    local port=9418 exec_path
    if nc -z localhost "$port"; then
      echo "Port $port should not have been open before this test's run of the git daemon!" >&2
      return 1
    fi
    exec_path="$(git --exec-path)"
    # TODO: Maybe dynamically set the `GIT_CONFIG_*` vars, to accommodate preexisting ones.
    GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=uploadpack.allowRefInWant GIT_CONFIG_VALUE_0=true \
      "$exec_path/git-daemon" --verbose --base-path=. --export-all --user-path &>/dev/null &
    daemon_pid=$!
    while ! nc -z localhost "$port"; do
      sleep 0.1
    done
    trap 'kill $daemon_pid' EXIT
}
