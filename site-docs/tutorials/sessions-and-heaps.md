# Tutorial: Building a Stateful Session

In this tutorial you will create variables across multiple executions, inspect heap snapshots, resume from specific points in history, and explore session tracking. By the end you will understand how mcp-v8 manages state across interactions.

## Prerequisites

- mcp-v8 installed ([Installation](installation.md))
- mcp-v8-cli installed
- Familiarity with stateful mode ([Execution Model](execution-model.md))

## Step 1: Start the server with local storage

```bash
mcp-v8 --http-port 3000 --directory-path /tmp/mcp-v8-tutorial-heaps
```

This stores heap snapshots in a local directory so they persist across server restarts.

## Step 2: Create a new session with initial state

Run your first piece of code. This creates a new session automatically:

```bash
mcp-v8-cli --http-port 3000 exec --code '
var items = [];
var sessionName = "shopping-list";
items.push("apples");
items;
'
```

Expected output includes the result `["apples"]` and an execution ID. Note the **heap hash** from the response -- you will need it in the next step.

## Step 3: Add more state to the session

Resume from the previous heap to add more items:

```bash
mcp-v8-cli --http-port 3000 exec --code '
items.push("bread");
items.push("milk");
items;
' --heap-id <HEAP_HASH_FROM_STEP_2>
```

Expected output:

```
["apples", "bread", "milk"]
```

## Step 4: Check the heap hash

Each execution produces a new heap snapshot with a unique hash. The hash is a content-addressable identifier for the exact V8 state at that point. You now have two snapshots:

1. After Step 2: `items = ["apples"]`
2. After Step 3: `items = ["apples", "bread", "milk"]`

## Step 5: Branch from an earlier state

One of the most powerful features of mcp-v8 is the ability to go back to any previous snapshot and branch from it. Resume from the Step 2 snapshot (not Step 3):

```bash
mcp-v8-cli --http-port 3000 exec --code '
items.push("cheese");
items.push("yogurt");
items;
' --heap-id <HEAP_HASH_FROM_STEP_2>
```

Expected output:

```
["apples", "cheese", "yogurt"]
```

Notice that "bread" and "milk" are not present -- you branched from the state where only "apples" existed. This creates a third snapshot on a separate branch.

## Step 6: List all sessions

View all sessions the server is tracking:

```bash
mcp-v8-cli --http-port 3000 list-sessions
```

This shows all sessions with their IDs and metadata.

## Step 7: List snapshots for a session

To see the history of snapshots in a session:

```bash
mcp-v8-cli --http-port 3000 list-session-snapshots --session-id <SESSION_ID>
```

This returns a list of all heap snapshots in the session, showing the tree of states you have built up.

## Step 8: List all executions

View all executions across all sessions:

```bash
mcp-v8-cli --http-port 3000 list-executions
```

Each execution entry shows:

- Execution ID
- Status (completed, running, failed)
- The heap hash of the resulting snapshot
- The session it belongs to

## Step 9: Resume from any snapshot

Pick any heap hash from the execution list and resume from it:

```bash
mcp-v8-cli --http-port 3000 exec --code '
JSON.stringify({ items, sessionName, count: items.length });
' --heap-id <ANY_HEAP_HASH>
```

The output will reflect the exact state at that snapshot.

## Step 10: Understand the snapshot tree

The snapshots form a tree structure:

```
[initial]
    |
    +-- Step 2: items = ["apples"]
         |
         +-- Step 3: items = ["apples", "bread", "milk"]
         |
         +-- Step 5: items = ["apples", "cheese", "yogurt"]
```

Each snapshot is immutable. You can always go back to any point and branch off in a new direction. This is particularly valuable for AI agents that want to try multiple approaches without losing previous work.

## What you learned

- Each code execution in stateful mode produces a heap snapshot with a unique hash
- You can resume from any previous snapshot by passing its heap hash
- Branching from earlier snapshots creates a tree of states
- Sessions group related executions together
- You can list sessions, snapshots, and executions to navigate history
- Snapshots are immutable -- previous states are never lost

Next, learn how to work with console output in [Working with Console Output](console-output.md).
