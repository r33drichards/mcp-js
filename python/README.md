# mcp-js Python library

```python
from mcp_js import Engine

with Engine(memory_mb=64, timeout_secs=2) as engine:
    result = engine.run_js("console.log(6 * 7)")
    print(result.output)  # 42
    if not result.ok:
        print(result.error)
```

The package loads its bundled UniFFI shared library directly. Import, engine
creation, execution, and close never invoke a shell, Cargo, or a server process.
JavaScript Promise awaiting is supported. `run_js` captures console output,
not an arbitrary final JavaScript expression's return value. JavaScript failures
and deadlines populate `result.error`; native API failures raise `EngineError`.
`result.artifacts` contains artifact metadata. One instance serializes calls;
`close()` is idempotent and context exit closes even if your application raises.

## Install locally

After preparing the package as described below:

```sh
uv pip install ./python
```

Stable releases built by the Python release workflow include a Linux x86-64
wheel in their GitHub release assets. Install the downloaded wheel with
`python -m pip install ./mcp_js-*.whl`. The wheel filename identifies its
manylinux compatibility. Local builds produce a platform-specific wheel.
macOS native builds are supported by the tooling and are unverified.

## Publish from GitHub Actions

The [Python release workflow](../.github/workflows/python-publish.yml) builds,
repairs, and smoke-tests the wheel, then attaches it to the stable GitHub
release. A manual run on a branch builds a wheel for testing.

To enable PyPI publishing, register a PyPI Trusted Publisher for `mcp-js`
with owner `r33drichards`, repository `mcp-js`, workflow
`python-publish.yml`, and environment `pypi`. Use a pending publisher for
the first release if the project has yet to be created. Set the GitHub
repository variable `PUBLISH_PYTHON_TO_PYPI` to `true`. The next stable release
can then publish the wheel to PyPI, where users can install it with
`python -m pip install mcp-js`.

## Build-time preparation (maintainers only)

Build the shared library using the existing native build instructions, then:

```sh
python scripts/prepare-python-package.py \
  --library target/python-uniffi/release/libmcp_v8_uniffi.so
uv build --wheel python
```

Preparation needs `uniffi-bindgen` 0.32.0 and Cargo on PATH. It copies generated
bindings into the private `mcp_js._bindings` module and bundles the corresponding
shared library. No generated bindings or compiled binaries are checked into Git.
Use a host-native shared library; do not package a cross-compiled library with
this host's wheel tag. Package builds fail if preparation has not been done.

The generated package is independent of the repository, build toolchain, and
original shared-library path at runtime. Test the installed wheel from outside
the checkout with `python -m unittest discover -s /path/to/python/tests`.
