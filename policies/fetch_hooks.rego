package mcp.fetch

# Example pre/post hooks for fetch(). Wire up with:
#
#   --policies-json '{
#     "fetch": {
#       "policies": [{"url": "file:///etc/policies/fetch.rego"}],
#       "pre":      [{"url": "file:///etc/policies/fetch_hooks.rego"}],
#       "post":     [{"url": "file:///etc/policies/fetch_hooks.rego"}]
#     }
#   }'
#
# A hook rule evaluates to a bool (allow/deny), or to an object:
#   {"allow": bool, "reason": string, "input"|"output": object}
# An undefined rule abstains (allow, no mutation). Pre hooks run in order,
# then the policies evaluate the effective (post-mutation) input.

# ── pre: upgrade http:// to https:// before the policy sees it ────────────

pre := {"input": patched} if {
	startswith(input.url, "http://")
	not credentials_in_query
	patched := object.union(input, {
		"url": concat("", ["https://", substring(input.url, 7, -1)]),
	})
}

credentials_in_query if {
	contains(lower(input.url_parsed.query), "api_key=")
}

# ── pre: refuse requests that smuggle credentials in the query string ─────

pre := {"allow": false, "reason": "credentials in query string"} if {
	credentials_in_query
}

# ── post: strip a sensitive header from responses before JS sees them ─────
# (object.union merges recursively, so replace the whole "headers" key rather
# than union-ing a filtered copy back in — the removed key would resurface.)

post := {"output": scrubbed} if {
	input.output.headers["x-internal-trace"]
	scrubbed := object.union(object.remove(input.output, ["headers"]), {
		"headers": object.remove(input.output.headers, ["x-internal-trace"]),
	})
}
