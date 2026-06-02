# Tutorial: Importing External Packages

In this tutorial you will learn how to import external packages in mcp-v8 using npm specifiers, JSR specifiers, URL imports, and dynamic imports. All external modules are resolved through esm.sh.

## Prerequisites

- mcp-v8 installed ([Installation](installation.md))
- mcp-v8-cli installed
- Server started: `mcp-v8 --http-port 3000`

## Step 1: Understand module resolution

mcp-v8 supports three import specifier formats:

| Format | Example | Description |
|---|---|---|
| `npm:` | `npm:lodash@4` | npm packages |
| `jsr:` | `jsr:@std/path@1` | JSR (JavaScript Registry) packages |
| `https://` | `https://esm.sh/lodash@4` | Direct URL imports |

All `npm:` and `jsr:` specifiers are rewritten to fetch from esm.sh, which serves npm and JSR packages as ES modules.

## Step 2: Import an npm package

Use the `npm:` prefix to import packages from npm:

```bash
mcp-v8-cli --http-port 3000 exec --code '
import _ from "npm:lodash@4";

const data = [1, [2, [3, [4]], 5]];
const flat = _.flattenDeep(data);
flat;
'
```

Expected output:

```
[1, 2, 3, 4, 5]
```

The `npm:lodash@4` specifier is rewritten to a URL like `https://esm.sh/lodash@4` before fetching.

## Step 3: Import with version pinning

You can pin to a specific version:

```bash
mcp-v8-cli --http-port 3000 exec --code '
import { format, parseISO } from "npm:date-fns@3.6.0";

const date = parseISO("2024-01-15");
format(date, "MMMM do, yyyy");
'
```

Expected output:

```
January 15th, 2024
```

## Step 4: Import from JSR

JSR is Deno's JavaScript Registry. Use the `jsr:` prefix:

```bash
mcp-v8-cli --http-port 3000 exec --code '
import { join } from "jsr:@std/path@1";

join("/home", "user", "documents", "file.txt");
'
```

Expected output:

```
/home/user/documents/file.txt
```

## Step 5: Import from a URL directly

You can also import directly from any URL that serves ES modules:

```bash
mcp-v8-cli --http-port 3000 exec --code '
import confetti from "https://esm.sh/canvas-confetti@1.9.2";

typeof confetti;
'
```

Expected output:

```
function
```

## Step 6: Import named exports

Import specific named exports from a package:

```bash
mcp-v8-cli --http-port 3000 exec --code '
import { camelCase, kebabCase, snakeCase } from "npm:lodash-es@4";

const results = {
  camel: camelCase("hello world"),
  kebab: kebabCase("hello world"),
  snake: snakeCase("hello world"),
};
results;
'
```

Expected output:

```json
{"camel": "helloWorld", "kebab": "hello-world", "snake": "hello_world"}
```

## Step 7: Use dynamic imports

Dynamic `import()` works for loading modules conditionally at runtime:

```bash
mcp-v8-cli --http-port 3000 exec --code '
const moduleName = "lodash";
const _ = await import(`npm:${moduleName}@4`);

_.default.chunk([1, 2, 3, 4, 5, 6], 2);
'
```

Expected output:

```
[[1, 2], [3, 4], [5, 6]]
```

Dynamic imports are useful when the module to load depends on runtime conditions.

## Step 8: Import multiple packages in one execution

You can import from multiple packages in a single execution:

```bash
mcp-v8-cli --http-port 3000 exec --code '
import _ from "npm:lodash@4";
import { format } from "npm:date-fns@3.6.0";

const dates = ["2024-01-01", "2024-06-15", "2024-12-31"];
const formatted = _.map(dates, d => format(new Date(d), "MMM d"));
formatted;
'
```

Expected output:

```
["Jan 1", "Jun 15", "Dec 31"]
```

## Step 9: Handle import errors

If a package does not exist or the version is invalid, you get a clear error:

```bash
mcp-v8-cli --http-port 3000 exec --code '
import { something } from "npm:nonexistent-package-xyz@999";
something;
'
```

The error message will indicate that the module could not be resolved.

## Step 10: Use imports with stateful sessions

Imports work with stateful sessions. Once a module is loaded in a heap snapshot, it is available when you resume:

```bash
# First execution: import and set up
mcp-v8-cli --http-port 3000 exec --code '
import _ from "npm:lodash@4";
var transform = (arr) => _.sortBy(arr);
transform([3, 1, 2]);
'
```

Note the heap hash, then resume:

```bash
# Second execution: use the imported module
mcp-v8-cli --http-port 3000 exec --code '
transform([9, 5, 7, 1]);
' --heap-id <HEAP_HASH>
```

Expected output:

```
[1, 5, 7, 9]
```

The lodash module and your `transform` function are both preserved in the heap.

## What you learned

- How to import npm packages with the `npm:` prefix
- How to import JSR packages with the `jsr:` prefix
- How to import directly from URLs
- That all specifiers are rewritten to esm.sh for resolution
- How to use dynamic `import()` for runtime module loading
- That imported modules persist across stateful session resumptions

Next, learn about running WebAssembly in [Running WebAssembly](webassembly.md).
