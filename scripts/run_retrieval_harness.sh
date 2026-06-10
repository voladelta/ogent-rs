#!/usr/bin/env bash
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HARNESS="$ROOT/evals/retrieval_harness.md"
RESULT_ROOT="$ROOT/eval-results/retrieval-harness"

if [[ ! -f "$HARNESS" ]]; then
  echo "missing harness: $HARNESS" >&2
  exit 1
fi

if [[ "$#" -gt 0 ]]; then
  profiles=("$@")
else
  profiles=(ds-flash ds-pro kimi)
fi

run_id="$(date -u +"%Y%m%dT%H%M%SZ")"
out_dir="$RESULT_ROOT/$run_id"
mkdir -p "$out_dir"

summary="$out_dir/summary.tsv"
printf "profile\texit_code\tself_grade\tstdout\tstderr\n" > "$summary"

echo "writing results to $out_dir"

for profile in "${profiles[@]}"; do
  stdout_file="$out_dir/${profile}.stdout.md"
  stderr_file="$out_dir/${profile}.stderr.log"
  prompt="$(printf 'Profile under test: %s\n\n' "$profile"; cat "$HARNESS")"

  echo "running $profile"
  (
    cd "$ROOT" &&
      cargo run -r -- --profile "$profile" -t "$prompt"
  ) >"$stdout_file" 2>"$stderr_file"
  code=$?

  self_grade="$(
    awk '
      {
        line = tolower($0)
        if (line ~ /self[-_ ]grade/) {
          scan = 4
        }
        if (scan > 0 && line ~ /(pass|partial|fail)/) {
          if (line ~ /partial/) print "partial"
          else if (line ~ /fail/) print "fail"
          else if (line ~ /pass/) print "pass"
          scan = 0
        }
        if (scan > 0) scan--
      }
    ' "$stdout_file" | tail -n 1
  )"
  if [[ -z "$self_grade" ]]; then
    self_grade="unknown"
  fi

  printf "%s\t%s\t%s\t%s\t%s\n" \
    "$profile" \
    "$code" \
    "$self_grade" \
    "$stdout_file" \
    "$stderr_file" >> "$summary"

  echo "$profile: exit=$code self_grade=$self_grade"
done

echo "summary: $summary"
