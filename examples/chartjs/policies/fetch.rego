# Allows only the GETs the PNG mode of chart.js needs: the resvg WASM binary
# and the font it rasterizes text with, both from unpkg.com.
package mcp.fetch

default allow = false

allow if {
    input.method == "GET"
    input.url_parsed.host == "unpkg.com"
}
