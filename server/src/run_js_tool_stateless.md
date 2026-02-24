run javascript or typescript code in v8

TypeScript support is type removal only — types are stripped before execution, not checked. Invalid types will be silently removed, not reported as errors.

params:
- code: the javascript or typescript code to run
- heap_memory_max_mb (optional): maximum V8 heap memory in megabytes (4–64, default: 8). Override the server default for this execution.
- execution_timeout_secs (optional): maximum execution time in seconds (1–300, default: 30). Override the server default for this execution.
- stdin (optional): a string to inject as the global variable `__stdin__`. Use this to pipe the output of a previous execution as input to the current one.

returns:
- output: the output of the javascript code (last expression value, `.toString()`'d)
- stdout (if non-empty): array of lines captured from `console.log` / `console.info`
- stderr (if non-empty): array of lines captured from `console.error` / `console.warn`


## Composing executions (piping output → input)

You can chain executions by passing the `output` of one call as the `stdin` of the next. Inside your code, read `__stdin__` to access the piped value:

```js
// Execution 1: produce output
JSON.stringify({ name: "Alice", score: 42 });
// → output: '{"name":"Alice","score":42}'

// Execution 2: consume output from execution 1 via stdin
var data = JSON.parse(__stdin__);
data.score * 2;
// → output: '84'
```


## Console output

`console.log(...)` and `console.info(...)` write to the `stdout` array.
`console.error(...)` and `console.warn(...)` write to the `stderr` array.

These are returned alongside the main output and can be used for debugging or side-channel data.


## Limitations

- **No `async`/`await` or Promises**: Asynchronous JavaScript is not supported. All code must be synchronous.
- **No `fetch` or network access**: There is no built-in way to make HTTP requests or access the network.
- **No file system access**: The runtime does not provide access to the local file system or environment variables.
- **No `npm install` or external packages**: You cannot install or import npm packages. Only standard JavaScript (ECMAScript) built-ins are available.
- **No timers**: Functions like `setTimeout` and `setInterval` are not available.
- **No DOM or browser APIs**: This is not a browser environment; there is no access to `window`, `document`, or other browser-specific objects.


The way the runtime works is that the main return value is the last expression evaluated. If you want the results of an execution, you must return it in the last line of code.


eg:

```js
const result = 1 + 1;
result;
```

would return:

```
2
```

you must also jsonify an object, and return it as a string to see its content.

eg:

```js
const obj = {
  a: 1,
  b: 2,
};
JSON.stringify(obj);
```

would return:

```
{"a":1,"b":2}
```


Each execution starts with a fresh V8 isolate — no state is carried between calls.
