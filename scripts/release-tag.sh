#!/usr/bin/env bash
# Release helper: bump version → commit → tag → push → trigger CI.
#
# Usage:
#   ./scripts/release-tag.sh                 # default: patch bump
#   ./scripts/release-tag.sh patch           # explicit patch bump (0.54.3 → 0.54.4)
#   ./scripts/release-tag.sh minor           # minor bump (0.54.3 → 0.55.0)
#   ./scripts/release-tag.sh major           # major bump (0.54.3 → 1.0.0)
#   ./scripts/release-tag.sh --dry-run       # show what would happen, no changes
#   ./scripts/release-tag.sh patch --dry-run # combine
#
# What it does:
#   1. Validates clean git working tree
#   2. Bumps version in package.json, tauri.conf.json, and both Cargo.toml files
#   3. Updates Cargo.lock files to reflect the new version
#   4. Commits the version bump
#   5. Creates a git tag (v<version>)
#   6. Pushes commit + tag to origin, triggering the Release workflow
#
# Prerequisites:
#   - Clean git working tree (no uncommitted changes)
#   - Node.js (for bump-version.js)
#   - cargo (for Cargo.lock update)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

RELEASE_TYPE="patch"
DRY_RUN=false
REMOTE="origin"

while [[ $# -gt 0 ]]; do
  case "$1" in
    patch|minor|major) RELEASE_TYPE="$1"; shift ;;
    --dry-run) DRY_RUN=true; shift ;;
    --remote) REMOTE="${2:-origin}"; shift 2 ;;
    -h|--help)
      sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) echo "Unknown option: $1" >&2; exit 1 ;;
  esac
done

# ── Preflight checks ────────────────────────────────────────────────────────

for cmd in node git cargo; do
  command -v "$cmd" >/dev/null 2>&1 || { echo "[release-tag] missing required command: $cmd" >&2; exit 1; }
done

CURRENT_VERSION="$(node -e "console.log(require('./app/package.json').version)")"
echo "[release-tag] current version: ${CURRENT_VERSION}"
echo "[release-tag] bump type: ${RELEASE_TYPE}"

# Check for uncommitted changes (allow untracked files).
if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "[release-tag] ERROR: working tree has uncommitted changes. Commit or stash them first." >&2
  git status --short >&2
  exit 1
fi

# ── Dry-run mode ─────────────────────────────────────────────────────────────

if [[ "$DRY_RUN" == true ]]; then
  # Compute next version without writing files.
  NEXT_VERSION="$(node -e "
    const v = '${CURRENT_VERSION}'.split('.').map(Number);
    if ('${RELEASE_TYPE}' === 'major') { v[0]++; v[1]=0; v[2]=0; }
    else if ('${RELEASE_TYPE}' === 'minor') { v[1]++; v[2]=0; }
    else { v[2]++; }
    console.log(v.join('.'));
  ")"
  echo "[release-tag] DRY RUN — would bump ${CURRENT_VERSION} → ${NEXT_VERSION}"
  echo "[release-tag] would commit: chore: release v${NEXT_VERSION}"
  echo "[release-tag] would tag: v${NEXT_VERSION}"
  echo "[release-tag] would push to: ${REMOTE} (commit + tag)"
  echo "[release-tag] CI workflow .github/workflows/release.yml triggers on tag push"
  exit 0
fi

# ── Step 1: Bump version ─────────────────────────────────────────────────────

echo "[release-tag] bumping version (${RELEASE_TYPE})..."
BUMP_OUTPUT="$(node scripts/release/bump-version.js "${RELEASE_TYPE}")"
NEXT_VERSION="$(echo "$BUMP_OUTPUT" | grep '^version=' | cut -d= -f2)"
TAG="$(echo "$BUMP_OUTPUT" | grep '^tag=' | cut -d= -f2)"

if [[ -z "$NEXT_VERSION" || -z "$TAG" ]]; then
  echo "[release-tag] ERROR: bump-version.js did not produce expected output" >&2
  echo "$BUMP_OUTPUT" >&2
  exit 1
fi

echo "[release-tag] new version: ${NEXT_VERSION} (tag: ${TAG})"

# ── Step 2: Update Cargo.lock files ──────────────────────────────────────────

echo "[release-tag] updating Cargo.lock files..."
# Root Cargo.lock — `cargo update` with --workspace refreshes the workspace
# member version without touching dependency versions.
cargo update --manifest-path Cargo.toml --workspace 2>/dev/null || cargo generate-lockfile --manifest-path Cargo.toml
# Tauri shell Cargo.lock
cargo update --manifest-path app/src-tauri/Cargo.toml --workspace 2>/dev/null || cargo generate-lockfile --manifest-path app/src-tauri/Cargo.toml

# ── Step 3: Commit ───────────────────────────────────────────────────────────

echo "[release-tag] committing version bump..."
git add \
  app/package.json \
  app/src-tauri/tauri.conf.json \
  app/src-tauri/Cargo.toml \
  app/src-tauri/Cargo.lock \
  Cargo.toml \
  Cargo.lock

git commit -m "chore: release v${NEXT_VERSION}"

# ── Step 4: Tag ──────────────────────────────────────────────────────────────

echo "[release-tag] creating tag ${TAG}..."
git tag -a "$TAG" -m "Release ${NEXT_VERSION}"

# ── Step 5: Push ─────────────────────────────────────────────────────────────

echo "[release-tag] pushing commit and tag to ${REMOTE}..."
git push "$REMOTE" HEAD
git push "$REMOTE" "$TAG"

echo ""
echo "═══════════════════════════════════════════════════════════════"
echo "  ✅ Released v${NEXT_VERSION}"
echo ""
echo "  Commit + tag pushed to ${REMOTE}."
echo "  CI release workflow should start automatically."
echo ""
echo "  Track progress:"
echo "    https://github.com/xinyuehtx/openhuman-dingtalk/actions"
echo ""
echo "  After CI finishes, the release will be at:"
echo "    https://github.com/xinyuehtx/openhuman-dingtalk/releases/tag/${TAG}"
echo "═══════════════════════════════════════════════════════════════"
