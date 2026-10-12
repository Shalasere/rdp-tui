#!/usr/bin/env bash
set -euo pipefail

release_tag=${GITHUB_REF_NAME:?Missing release tag}
release_repo=${GH_REPO:?Missing release repository}
python scripts/release.py version --tag "$release_tag"
python scripts/release.py checksums --require-all
(cd dist && sha256sum -c SHA256SUMS)

prerelease=false
latest=true
if [[ $release_tag == *-* ]]; then
  prerelease=true
  latest=false
fi

if gh release view "$release_tag" --repo "$release_repo" --json isDraft >/dev/null 2>&1; then
  is_draft=$(gh release view "$release_tag" --repo "$release_repo" --json isDraft --jq .isDraft)
  if [[ $is_draft != true ]]; then
    # Published binaries are immutable. A rerun may succeed only if they match.
    verification_dir=$(mktemp -d)
    trap 'rm -rf "$verification_dir"' EXIT
    gh release download "$release_tag" --repo "$release_repo" --pattern SHA256SUMS --dir "$verification_dir"
    cmp dist/SHA256SUMS "$verification_dir/SHA256SUMS"
    exit 0
  fi
else
  gh release create "$release_tag" --repo "$release_repo" --verify-tag --draft \
    --title "$release_tag" --generate-notes --prerelease="$prerelease"
fi

gh release upload "$release_tag" --repo "$release_repo" dist/*.tar.gz dist/SHA256SUMS --clobber
gh release edit "$release_tag" --repo "$release_repo" --draft=false --prerelease="$prerelease" --latest="$latest"
