#!/usr/bin/env bash
# Fork-release helper for openhuman-dingtalk.
#
# Builds an UNSIGNED macOS Apple Silicon (arm64) bundle and uploads the
# `.dmg` to a GitHub Release on this fork. Run on a macOS arm64 machine.
#
# Windows x64 builds: see scripts/release-fork.ps1 — run that on a
# Windows x64 machine and pass --tag matching the one this script created.
#
# Requirements:
#   - macOS arm64 (Apple Silicon)
#   - `cargo tauri` (vendored CEF-aware CLI; pnpm dev:app installs it)
#   - `gh` (GitHub CLI) authenticated against the fork
#   - `jq`
#
# Usage:
#   ./scripts/release-fork.sh               # build + upload using version from app/package.json
#   ./scripts/release-fork.sh --dry-run     # build only, skip upload
#   ./scripts/release-fork.sh --tag v0.54.3 # build + upload to an existing release tag
#   ./scripts/release-fork.sh --skip-build  # skip cargo build, only upload existing artifacts

set -euo pipefail

REPO_DEFAULT="xinyuehtx/openhuman-dingtalk"
REPO="${RELEASE_FORK_REPO:-$REPO_DEFAULT}"

DRY_RUN=false
SKIP_BUILD=false
TAG_OVERRIDE=""
NOTES_OVERRIDE=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=true; shift ;;
    --skip-build) SKIP_BUILD=true; shift ;;
    --tag) TAG_OVERRIDE="${2:-}"; shift 2 ;;
    --notes) NOTES_OVERRIDE="${2:-}"; shift 2 ;;
    -h|--help)
      sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) echo "Unknown option: $1" >&2; exit 1 ;;
  esac
done

OS_RAW="$(uname -s)"
ARCH_RAW="$(uname -m)"
if [[ "$OS_RAW" != "Darwin" || "$ARCH_RAW" != "arm64" ]]; then
  echo "[release-fork] This script targets macOS arm64. Detected: ${OS_RAW}/${ARCH_RAW}" >&2
  echo "[release-fork] For Windows x64, run scripts/release-fork.ps1 on a Windows machine." >&2
  exit 1
fi

for cmd in jq gh cargo pnpm; do
  command -v "$cmd" >/dev/null 2>&1 || { echo "[release-fork] missing required command: $cmd" >&2; exit 1; }
done

VERSION="$(jq -r .version app/package.json)"
if [[ -z "$VERSION" || "$VERSION" == "null" ]]; then
  echo "[release-fork] could not read version from app/package.json" >&2
  exit 1
fi

TAG="${TAG_OVERRIDE:-v$VERSION}"
TARGET="aarch64-apple-darwin"
BUNDLE_DIR="target/${TARGET}/release/bundle"

echo "[release-fork] repo=${REPO} version=${VERSION} tag=${TAG} target=${TARGET}"

# --dry-run is a "what would happen?" preflight. Skip the (slow) build and
# the upload; just check env, gh auth, and what artifact path we'd look for.
if [[ "$DRY_RUN" == true ]]; then
  echo "[release-fork] DRY RUN — skipping cargo build and gh upload"
  if gh auth status --hostname github.com >/dev/null 2>&1; then
    echo "[release-fork] gh auth: ok"
  else
    echo "[release-fork] gh auth: NOT authenticated — run 'gh auth login' before a real release"
  fi
  EXPECTED_DMG="${BUNDLE_DIR}/dmg/OpenHuman_${VERSION}_aarch64.dmg"
  echo "[release-fork] would build: cargo tauri build --target ${TARGET} --bundles app dmg"
  echo "[release-fork] would look for artifact at: ${EXPECTED_DMG}"
  echo "[release-fork] would upload to: ${REPO}@${TAG}"
  exit 0
fi

if [[ "$SKIP_BUILD" == false ]]; then
  echo "[release-fork] running cargo tauri build (unsigned, dmg only)"
  # `--bundles app dmg` skips deb / appimage / nsis / msi which would either
  # fail or be useless on macOS arm64.
  (cd app && pnpm tauri:ensure)
  cargo tauri build \
    --config app/src-tauri/tauri.conf.json \
    --target "$TARGET" \
    --bundles app dmg
else
  echo "[release-fork] --skip-build set; using existing artifacts under ${BUNDLE_DIR}"
fi

DMG_PATH="$(find "${BUNDLE_DIR}/dmg" -maxdepth 1 -name 'OpenHuman_*_aarch64.dmg' -type f | head -n 1 || true)"
if [[ -z "$DMG_PATH" || ! -f "$DMG_PATH" ]]; then
  echo "[release-fork] could not find OpenHuman_*_aarch64.dmg under ${BUNDLE_DIR}/dmg" >&2
  echo "[release-fork] tip: run without --skip-build, or check the build output above for errors" >&2
  exit 1
fi
DMG_NAME="$(basename "$DMG_PATH")"
DMG_SHA256="$(shasum -a 256 "$DMG_PATH" | awk '{print $1}')"

echo "[release-fork] artifact: ${DMG_NAME} (${DMG_SHA256})"

# Create the release if it doesn't exist; otherwise reuse it.
if ! gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; then
  echo "[release-fork] creating draft release ${TAG} on ${REPO}"
  notes="${NOTES_OVERRIDE:-Fork build of OpenHuman ${VERSION}. Unsigned macOS arm64 bundle.}"
  gh release create "$TAG" \
    --repo "$REPO" \
    --title "OpenHuman 钉钉 ${VERSION}" \
    --notes "$notes" \
    --draft
else
  echo "[release-fork] reusing existing release ${TAG} on ${REPO}"
fi

echo "[release-fork] uploading ${DMG_NAME}"
gh release upload "$TAG" "$DMG_PATH" --repo "$REPO" --clobber

# Also upload a sha256 sidecar so install.sh can verify integrity even
# without a latest.json manifest.
SHA_FILE="${DMG_PATH}.sha256"
printf '%s  %s\n' "$DMG_SHA256" "$DMG_NAME" > "$SHA_FILE"
gh release upload "$TAG" "$SHA_FILE" --repo "$REPO" --clobber

echo "[release-fork] done."
echo "[release-fork] release page: https://github.com/${REPO}/releases/tag/${TAG}"
echo "[release-fork] note: release is still a DRAFT — review and publish via the GitHub UI or:"
echo "    gh release edit ${TAG} --repo ${REPO} --draft=false"
