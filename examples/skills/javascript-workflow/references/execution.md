Call `run_js` with the JavaScript code. Save the returned execution ID.
Poll `get_execution` until it reaches a terminal status, then inspect the result.
Read `get_execution_output` when you need console output. Use `cancel_execution`
if an execution is no longer needed.
