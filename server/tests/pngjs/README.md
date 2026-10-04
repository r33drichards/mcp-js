# pngjs regression fixture

`pngjs-7.0.0.mjs` is the unmodified ES module build fetched from
https://esm.sh/pngjs@7.0.0/es2022/pngjs.mjs. Its SHA-256 is
`9bc1746a94edd28a086be39e1af221f1933ecd82551c0136e90fbb225edd8516`.
The package's MIT license is included in `LICENSE`.

The offline test supplies only this package source in the virtual module map.
Its `/node/*.mjs` imports use the real loader's embedded-builtin bridge, so no
network downloads are needed. The optional live test uses the exact npm import
and current CDN graph:

```sh
cargo test --test pngjs -- --test-threads=1
cargo test --test pngjs -- --ignored --test-threads=1
```

Coverage includes synchronous and asynchronous PNG encoding/decoding, multiple
output chunks, a tiny image, pixel equality, invalid CRC rejection, zlib options,
truncated compression data, and policy enforcement on CDN builtin aliases.
