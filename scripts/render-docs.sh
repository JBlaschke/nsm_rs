#!/usr/bin/env sh
# Render the markdown documentation (README.md and docs/**/*.md) to HTML with
# pandoc, keeping the directory layout and turning links to .md files into
# links to the rendered pages. Used by the Pages workflow; run it locally to
# check a change:
#
#   scripts/render-docs.sh target/doc/guide
#
set -eu
out=${1:?usage: render-docs.sh OUTPUT_DIR}
command -v pandoc >/dev/null || { echo "pandoc is required" >&2; exit 1; }
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
filter=$(mktemp)
trap 'rm -f "$filter"' EXIT
repo=${NSM_REPO_URL:-https://github.com/JBlaschke/nsm_rs/blob/main}
cat > "$filter" <<LUA
-- Point links at the rendered pages instead of the markdown sources, and
-- send links to other repository files (Dockerfile, compose.yaml, ...) to
-- the repository itself.
function Link(el)
  local t = el.target
  if t:match("^%a[%w+.-]*:") or t:match("^#") then return el end
  if t:match("%.md$") or t:match("%.md#") then
    el.target = t:gsub("%.md$", ".html"):gsub("%.md#", ".html#")
  else
    el.target = "$repo/" .. t:gsub("^%./", "")
  end
  return el
end
LUA

# render SOURCE.md RELATIVE/PATH.html
render() {
  src=$1; rel=$2
  mkdir -p "$(dirname -- "$out/$rel")"
  title=$(sed -n 's/^# //p' "$src" | head -n 1)
  # The stylesheet sits at the output root; climb one level per directory.
  css=style.css; dir=$(dirname -- "$rel")
  while [ "$dir" != "." ]; do css="../$css"; dir=$(dirname -- "$dir"); done
  pandoc --standalone --from gfm --to html5 --lua-filter "$filter" \
    --metadata "title=${title:-$(basename -- "$src")}" --metadata lang=en \
    --css "$css" --output "$out/$rel" "$src"
}

cd -- "$root"
mkdir -p "$out"
cat > "$out/style.css" <<'CSS'
body { max-width: 52rem; margin: 2rem auto; padding: 0 1rem; font: 16px/1.5 system-ui, sans-serif; color: #222; }
pre { background: #f4f4f4; padding: .75rem; overflow-x: auto; }
code { font-size: 90%; }
table { border-collapse: collapse; } th, td { border: 1px solid #ccc; padding: .25rem .5rem; text-align: left; vertical-align: top; }
CSS
for md in README.md CHANGELOG.md CONTRIBUTING.md deploy/k8s/README.md; do
  render "$md" "${md%.md}.html"
done
find docs -name '*.md' | sort | while read -r md; do
  render "$md" "${md%.md}.html"
done
echo "rendered $(find "$out" -name '*.html' | wc -l | tr -d ' ') pages into $out"
