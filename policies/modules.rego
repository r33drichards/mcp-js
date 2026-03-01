package mcp.modules

default allow = false

# Allow specific npm packages (via esm.sh)
allow if {
    input.specifier_type == "npm"
    npm_package_allowed
}

# Allow specific JSR packages (via esm.sh/jsr/)
allow if {
    input.specifier_type == "jsr"
    jsr_package_allowed
}

# Allow specific URL hosts
allow if {
    input.specifier_type == "url"
    url_host_allowed
}

# ── Allowed npm packages ────────────────────────────────────────────────
# Add package names (as they appear on esm.sh) to this set.
# The policy checks that the resolved URL host is esm.sh and the path
# starts with one of these package prefixes.

allowed_npm_packages := {
    "lodash-es",
    "uuid",
    "date-fns",
    "zod",
}

npm_package_allowed if {
    input.url_parsed.host == "esm.sh"
    some pkg in allowed_npm_packages
    startswith(input.url_parsed.path, sprintf("/%s", [pkg]))
}

# ── Allowed JSR packages ────────────────────────────────────────────────

allowed_jsr_packages := {
    "@luca/cases",
    "@std/path",
}

jsr_package_allowed if {
    input.url_parsed.host == "esm.sh"
    some pkg in allowed_jsr_packages
    startswith(input.url_parsed.path, sprintf("/jsr/%s", [pkg]))
}

# ── Allowed URL hosts ───────────────────────────────────────────────────
# Direct URL imports are allowed from these exact hosts.

allowed_url_hosts := {
    "esm.sh",
    "cdn.jsdelivr.net",
    "unpkg.com",
}

url_host_allowed if {
    allowed_url_hosts[input.url_parsed.host]
}

# Wildcard suffix matches for URL hosts
allowed_url_suffixes := {
    ".esm.sh",
}

url_host_allowed if {
    some suffix in allowed_url_suffixes
    endswith(input.url_parsed.host, suffix)
}
