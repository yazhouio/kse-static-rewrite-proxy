#!/bin/sh

commit_message_file=$1
subject=$(sed -n '1p' "$commit_message_file")
conventional_pattern='^[[:alnum:]][[:alnum:]_-]*(\([^)]+\))?!?: [^[:space:]].*$'

if printf '%s\n' "$subject" | grep -Eq "$conventional_pattern"; then
  exit 0
fi

cat >&2 <<'EOF'
Commit message must follow Conventional Commits:
  <type>[optional scope][optional !]: <description>

Examples:
  feat(proxy): support compressed responses
  fix!: reject invalid upstream addresses
EOF
exit 1
