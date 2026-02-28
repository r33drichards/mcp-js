package mcp.fetch

default allow = false

# Read-only: GET and HEAD only
allow if {
    input.method == "GET"
    domain_allowed
}

allow if {
    input.method == "HEAD"
    domain_allowed
}

# Exact domain matches
exact_domains := {
    "github.com",
    "api.github.com",
    "bedrock-runtime.us-west-2.amazonaws.com",
    "bedrock.us-west-2.amazonaws.com",
    "sts.amazonaws.com",
    "sts.us-west-2.amazonaws.com",
    "otel.cua.ai",
    "cache.nixos.org",
    "channels.nixos.org",
    "nixos.org",
    "proxy.golang.org",
    "sum.golang.org",
    "gopkg.in",
    "golang.org",
    "google.golang.org",
    "files.pythonhosted.org",
    "pypi.org",
    "registry.npmjs.org",
}

# Wildcard suffix matches (*.example.com)
wildcard_suffixes := {
    ".github.com",
    ".githubusercontent.com",
    ".cachix.org",
}

domain_allowed if {
    exact_domains[input.url_parsed.host]
}

domain_allowed if {
    some suffix in wildcard_suffixes
    endswith(input.url_parsed.host, suffix)
}
