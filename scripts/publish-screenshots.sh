#!/usr/bin/env bash
# Publish a gallery as a retained commit. Print its SHA on stdout.
set -euo pipefail

: "${SCREENSHOTS_REMOTE:?}"
: "${SCREENSHOTS_DIR:?}"
: "${GITHUB_SHA:?}"
: "${GITHUB_SERVER_URL:?}"
: "${GITHUB_REPOSITORY:?}"
: "${GITHUB_RUN_ID:?}"
: "${GITHUB_RUN_ATTEMPT:?}"

preview="$(mktemp -d)"
trap 'rm -rf "$preview"' EXIT
cp "$SCREENSHOTS_DIR"/*.png "$preview/"

names=(
  perseid-main.png perseid-project.png perseid-project-dialog.png
  perseid-proxy-websockets.png perseid-target-site-map.png perseid-target-scope.png
  perseid-proxy-intercept.png perseid-proxy-match-replace.png perseid-proxy-tls.png
  perseid-intruder.png perseid-repeater.png perseid-repeater-websockets.png perseid-decoder.png
  perseid-comparer.png perseid-comparer-result.png perseid-sequencer.png
  perseid-sequencer-result.png perseid-search.png perseid-settings.png perseid-cookie-jar.png perseid-memory.png
)
labels=(
  'Proxy - HTTP history' 'Projects - File menu' 'Projects - Save dialog'
  'Proxy - WebSockets history' 'Target - Site map' 'Target - Scope'
  'Proxy - Intercept' 'Proxy - Match/replace' 'Proxy - Proxy settings'
  'Intruder' 'Repeater' 'Repeater - WebSockets' 'Decoder' 'Comparer' 'Comparer - Result'
  'Sequencer' 'Sequencer - Result' 'Search' 'Settings' 'Cookie jar' 'Memory usage'
)
{
  echo '# Perseid screenshot gallery'
  echo
  echo "Generated from \`$GITHUB_SHA\` by the [Screenshots workflow]($GITHUB_SERVER_URL/$GITHUB_REPOSITORY/actions/runs/$GITHUB_RUN_ID) (attempt $GITHUB_RUN_ATTEMPT)."
  echo
  for i in "${!names[@]}"; do
    echo "## ${labels[$i]}"
    echo
    # Relative paths resolve against the gallery's commit, not the moving branch.
    echo "![${labels[$i]}](${names[$i]})"
    echo
  done
} > "$preview/index.md"

cd "$preview"
git init -q
git config user.name 'github-actions[bot]'
git config user.email '41898282+github-actions[bot]@users.noreply.github.com'
git remote add origin "$SCREENSHOTS_REMOTE"

# Another run can publish between our fetch and push. Recreate our snapshot on
# its new parent and retry, keeping both commits in the branch history.
for attempt in 1 2 3 4 5; do
  refs="$(git ls-remote --heads origin refs/heads/screenshots-preview)"
  if [[ -n "$refs" ]]; then
    git fetch -q origin refs/heads/screenshots-preview
    git reset -q --mixed FETCH_HEAD
  fi
  git add -A
  git commit -q --allow-empty -m "Screenshots from $GITHUB_SHA"
  if git push -q origin HEAD:refs/heads/screenshots-preview; then
    git rev-parse HEAD
    exit 0
  fi
  echo "Screenshot publish attempt $attempt failed; retrying." >&2
done
echo 'Could not publish screenshots after five attempts.' >&2
exit 1
