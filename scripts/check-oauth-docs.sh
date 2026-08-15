#!/usr/bin/env bash
set -euo pipefail

repo_root=${1:-.}

assert_contains() {
  local file=$1
  local expected=$2

  if ! tr '\n' ' ' < "$repo_root/$file" | grep -Fq -- "$expected"; then
    printf 'OAuth documentation contract missing from %s:\n  %s\n' "$file" "$expected" >&2
    exit 1
  fi
}

assert_contains README.md '"type": "oauth_browser"'
assert_contains README.md 'Protected-resource, authorization, token, and dynamic-registration endpoints require HTTPS unless they are loopback endpoints.'
assert_contains README.md 'OAuth credentials are resolved on each initial connection or reconnect.'
assert_contains README.md 'An established transport keeps its bearer token until reconnect or invalidity.'
assert_contains README.md 'mode `0600`'
assert_contains README.md 'When `redirect_port` is omitted'
assert_contains server/README.md 'headless machine'
assert_contains server/README.md 'http://localhost:<port>/callback'
assert_contains server/README.md 'files owned by another user'
assert_contains server/README.md 'files readable by group or others'
assert_contains server/README.md 'Unsafe, wrong-owner, or symlinked cache files are never consumed.'
assert_contains server/README.md 'successful authorization may replace the cache securely.'
assert_contains server/README.md 'revoke the grant at the authorization provider and delete `token_cache`'
assert_contains site-docs/reference/cli-flags.md '`oauth_browser` is supported for HTTP only and is accepted only in JSON.'
assert_contains site-docs/reference/cli-flags.md 'cached refresh tokens renew access on connection or reconnect without opening a browser.'
assert_contains site-docs/reference/cli-flags.md 'Protected-resource and discovered OAuth endpoints require HTTPS unless loopback.'
