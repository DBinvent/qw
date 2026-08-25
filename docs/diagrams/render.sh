#!/usr/bin/env bash
# Render every .eraserdiagram in this directory through Eraser's API.
#
#   ERASER_API_KEY=… ./render.sh              # all of them -> ./out/*.svg
#   ERASER_API_KEY=… ./render.sh joining      # just one
#   ./render.sh --check                       # no key needed: list what would render
#
# The API is https://api.eraser.io/render/elements — POST the diagram text plus
# a diagramType, get back a URL to the rendered image. Keys come from an Eraser
# workspace (Settings -> API), and the endpoint is a paid-plan feature; without
# one, paste a file's contents into a new diagram at app.eraser.io instead,
# which costs nothing and renders the same DSL.
set -euo pipefail
cd "$(dirname "$0")"

# Eraser needs to be told which renderer to use; it is not inferred from the
# text. Keep this table in step with the files.
declare -A TYPE=(
  [joining]=sequence-diagram
  [job-lifecycle]=sequence-diagram
  [referral-routing]=flowchart-diagram
)

FORMAT="${FORMAT:-svg}"   # svg | png
THEME="${THEME:-dark}"    # matches knownby.work's palette
SCALE="${SCALE:-2}"

targets=()
if [ "${1:-}" = "--check" ]; then
  for f in *.eraserdiagram; do
    n="${f%.eraserdiagram}"
    printf '%-20s %s\n' "$n" "${TYPE[$n]:-UNMAPPED — add it to TYPE in render.sh}"
  done
  exit 0
elif [ $# -gt 0 ]; then
  targets=("$@")
else
  for f in *.eraserdiagram; do targets+=("${f%.eraserdiagram}"); done
fi

: "${ERASER_API_KEY:?set ERASER_API_KEY (Eraser workspace -> Settings -> API)}"
mkdir -p out

for name in "${targets[@]}"; do
  file="$name.eraserdiagram"
  [ -f "$file" ] || { echo "no such diagram: $file" >&2; exit 1; }
  type="${TYPE[$name]:-}"
  [ -n "$type" ] || { echo "$name has no diagramType — add it to TYPE in render.sh" >&2; exit 1; }

  # The DSL is JSON-encoded rather than interpolated: it contains quotes,
  # backslash-n and braces, all of which break a hand-built payload.
  payload=$(python3 -c '
import json, sys
print(json.dumps({
    "text": open(sys.argv[1]).read(),
    "diagramType": sys.argv[2],
    "background": True,
    "theme": sys.argv[3],
    "scale": sys.argv[4],
    "returnFile": False,
}))' "$file" "$type" "$THEME" "$SCALE")

  echo "==> $name ($type)"
  resp=$(curl -sS -X POST https://api.eraser.io/render/elements \
    -H "Authorization: Bearer $ERASER_API_KEY" \
    -H 'Content-Type: application/json' \
    --data "$payload")

  url=$(printf '%s' "$resp" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("imageUrl",""))' 2>/dev/null || true)
  if [ -z "$url" ]; then
    echo "    failed: $resp" >&2
    exit 1
  fi
  curl -sS -o "out/$name.$FORMAT" "$url"
  echo "    out/$name.$FORMAT"
done
