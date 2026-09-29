#!/usr/bin/env bash
# Enforces the AGENTS.md rule that production modules stay below 500 lines.
#
# Test code is exempt. A module listed in module-size-exceptions must still be
# at or over the limit, so the list only shrinks as modules are split.
set -euo pipefail

cd "$(dirname "$0")/.."
limit=500
exceptions=scripts/module-size-exceptions
status=0

listed=$(grep -v '^#' "$exceptions" | awk 'NF { print $1 }')

while IFS= read -r file; do
  lines=$(wc -l < "$file" | tr -d ' ')
  if [ "$lines" -ge "$limit" ] && ! grep -qxF "$file" <<< "$listed"; then
    echo "$file has $lines lines; production modules stay below $limit." \
      "Name its second responsibility and move it out, or delete something." >&2
    status=1
  fi
done < <(
  git ls-files '*.rs' '*.ts' '*.tsx' '*.mjs' '*.js' |
    grep -vE '(^|/)tests?(/|\.rs$)|_tests?\.rs$|\.test\.(ts|tsx|mjs|js)$|/benches/|/examples/|/dist/|\.d\.ts$|(^|/)test[_-]|/fixtures?/'
)

for file in $listed; do
  if [ ! -f "$file" ]; then
    echo "$exceptions lists $file, which no longer exists; remove it." >&2
    status=1
  elif [ "$(wc -l < "$file" | tr -d ' ')" -lt "$limit" ]; then
    echo "$exceptions lists $file, which is now below $limit lines; remove it." >&2
    status=1
  fi
done

exit "$status"
