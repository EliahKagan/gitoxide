#!/usr/bin/env bash
set -eu -o pipefail

cat >trivial <<'EOF'
#!/bin/sh
printf 'Hello, world!\n'
EOF

cat >name-and-args <<'EOF'
#!/bin/sh
set -u
printf '%s\n' "$0" "$@"
EOF

chmod +x trivial name-and-args
