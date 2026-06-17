package mcp.modules

default allow = false


allow if {
    input.specifier_type == "npm"
    npm_package_allowed
}


allow if {
    input.specifier_type == "jsr"
    jsr_package_allowed
}


allow if {
    input.specifier_type == "url"
    url_host_allowed
}






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



allowed_jsr_packages := {
    "@luca/cases",
    "@std/path",
}

jsr_package_allowed if {
    input.url_parsed.host == "esm.sh"
    some pkg in allowed_jsr_packages
    startswith(input.url_parsed.path, sprintf("/jsr/%s", [pkg]))
}




allowed_url_hosts := {
    "esm.sh",
    "cdn.jsdelivr.net",
    "unpkg.com",
}

url_host_allowed if {
    allowed_url_hosts[input.url_parsed.host]
}


allowed_url_suffixes := {
    ".esm.sh",
}

url_host_allowed if {
    some suffix in allowed_url_suffixes
    endswith(input.url_parsed.host, suffix)
}
