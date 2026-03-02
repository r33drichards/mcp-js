# Architecture

This document describes the architecture of **mcp-js** — an MCP (Model Context Protocol) server that provides a sandboxed JavaScript/TypeScript execution environment. The server is written in Rust, embeds a V8 engine via `deno_core`, and supports stateful heap persistence, clustering, WASM modules, policy-gated fetch/fs, and external MCP server integration.

## High-Level Overview

```mermaid
graph TB
    Client["MCP Client<br/>(LLM Agent / HTTP Client)"]

    subgraph Transports["Transport Layer"]
        STDIO["stdio"]
        SSE["HTTP+SSE<br/>(legacy)"]
        HTTP["Streamable HTTP<br/>(MCP 2025-03-26+)"]
    end

    subgraph MCP["MCP Service Layer"]
        Stateful["McpService<br/>(stateful)"]
        Stateless["StatelessMcpService<br/>(stateless)"]
    end

    API["REST API<br/>/api/exec, /api/executions"]

    subgraph EngineCore["Engine Core"]
        Engine["Engine"]
        V8["V8 Isolate<br/>(deno_core)"]
        TS["TypeScript Transpiler<br/>(SWC)"]
        WASM["WASM Runtime"]
    end

    subgraph Extensions["V8 Extensions (deno_core ops)"]
        Console["console"]
        Fetch["fetch()"]
        FS["fs.*"]
        McpClient["mcp.*"]
        ModLoader["Module Loader<br/>(npm/jsr/URL)"]
    end

    subgraph Policy["Policy Engine"]
        OPA["OPA Policy Chain"]
        LocalRego["Local Rego<br/>(regorus)"]
        RemoteOPA["Remote OPA<br/>(REST API)"]
    end

    subgraph Storage["Storage Layer"]
        HeapStore["Heap Storage"]
        FileStore["FileHeapStorage"]
        S3Store["S3HeapStorage"]
        CacheStore["WriteThroughCache"]
        SessionLog["Session Log<br/>(sled)"]
        HeapTags["Heap Tag Store<br/>(sled)"]
        ExecReg["Execution Registry<br/>(sled + DashMap)"]
    end

    subgraph Cluster["Cluster Layer (Raft)"]
        ClusterNode["ClusterNode"]
        Raft["Raft Consensus"]
        RaftHTTP["Raft HTTP Server"]
    end

    Client --> STDIO & SSE & HTTP
    STDIO & SSE & HTTP --> Stateful & Stateless
    HTTP --> API
    SSE --> API
    Stateful & Stateless --> Engine
    API --> Engine
    Engine --> V8
    Engine --> TS
    Engine --> WASM
    V8 --> Console & Fetch & FS & McpClient & ModLoader
    Fetch --> OPA
    FS --> OPA
    ModLoader --> OPA
    OPA --> LocalRego & RemoteOPA
    Engine --> HeapStore & SessionLog & HeapTags & ExecReg
    HeapStore --> FileStore & S3Store & CacheStore
    SessionLog --> ClusterNode
    HeapTags --> ClusterNode
    ClusterNode --> Raft --> RaftHTTP
```

## Module Structure

The server is a single Rust crate with four top-level modules and ten engine submodules:

```mermaid
graph LR
    subgraph "server (crate root)"
        main["main.rs<br/>CLI parsing, wiring"]
        lib["lib.rs<br/>pub mod exports"]
    end

    subgraph "Top-Level Modules"
        mcp["mcp.rs<br/>MCP tool handlers"]
        api["api.rs<br/>REST API routes"]
        cluster["cluster.rs<br/>Raft consensus"]
    end

    subgraph "engine/"
        mod_rs["mod.rs<br/>Engine struct, V8 execution"]
        console["console.rs<br/>console.log capture"]
        execution["execution.rs<br/>Execution registry"]
        fetch["fetch.rs<br/>fetch() implementation"]
        fs_mod["fs.rs<br/>Filesystem ops"]
        heap_storage["heap_storage.rs<br/>Heap snapshot storage"]
        heap_tags["heap_tags.rs<br/>Heap tag metadata"]
        session_log["session_log.rs<br/>Session logging"]
        opa["opa.rs<br/>OPA policy evaluation"]
        module_loader["module_loader.rs<br/>ES module imports"]
        mcp_client["mcp_client.rs<br/>External MCP servers"]
    end

    main --> mcp & api & cluster & mod_rs
    mcp --> mod_rs
    api --> mod_rs
    mod_rs --> console & execution & fetch & fs_mod & heap_storage & heap_tags & session_log & opa & module_loader & mcp_client
    session_log --> cluster
    heap_tags --> cluster
    fetch --> opa
    fs_mod --> opa
    module_loader --> opa
```

## Module Dependency Graph

```mermaid
graph TD
    main["main.rs"] --> engine["engine::mod"]
    main --> mcp["mcp"]
    main --> api["api"]
    main --> cluster["cluster"]
    main --> engine_fetch["engine::fetch"]
    main --> engine_fs["engine::fs"]
    main --> engine_exec["engine::execution"]
    main --> engine_opa["engine::opa"]
    main --> engine_heap_storage["engine::heap_storage"]
    main --> engine_heap_tags["engine::heap_tags"]
    main --> engine_session_log["engine::session_log"]
    main --> engine_module_loader["engine::module_loader"]
    main --> engine_mcp_client["engine::mcp_client"]

    mcp --> engine
    mcp --> engine_heap_tags
    api --> engine

    engine --> engine_console["engine::console"]
    engine --> engine_exec
    engine --> engine_heap_storage
    engine --> engine_heap_tags
    engine --> engine_session_log
    engine --> engine_fetch
    engine --> engine_fs
    engine --> engine_module_loader
    engine --> engine_mcp_client

    engine_fetch --> engine_opa
    engine_fs --> engine_opa
    engine_module_loader --> engine_opa

    engine_session_log --> cluster
    engine_heap_tags --> cluster

    classDef entrypoint fill:#f96,stroke:#333
    classDef core fill:#6cf,stroke:#333
    classDef ext fill:#9f9,stroke:#333
    classDef storage fill:#ff9,stroke:#333
    classDef policy fill:#f9f,stroke:#333

    class main entrypoint
    class engine,mcp,api core
    class engine_console,engine_fetch,engine_fs,engine_module_loader,engine_mcp_client ext
    class engine_exec,engine_heap_storage,engine_heap_tags,engine_session_log storage
    class engine_opa,cluster policy
```

## Module Descriptions

### Top-Level Modules

| Module | File | Description |
|--------|------|-------------|
| `main` | `main.rs` | Entry point. Parses CLI args (via `clap`), initializes V8, builds the `Engine`, configures policy chains, WASM modules, MCP clients, and starts the chosen transport (stdio/SSE/Streamable HTTP). |
| `mcp` | `mcp.rs` | MCP protocol handlers. Defines `McpService` (stateful, 11 tools) and `StatelessMcpService` (stateless, 1 tool) implementing `rmcp::ServerHandler`. |
| `api` | `api.rs` | REST API router (axum). Exposes `/api/exec`, `/api/executions`, `/api/executions/{id}`, `/api/executions/{id}/output`, `/api/executions/{id}/cancel`. Merged into the MCP transport when using HTTP. |
| `cluster` | `cluster.rs` | Raft-inspired cluster consensus. Implements leader election, log replication, and a replicated key-value store backed by sled. Each node runs an HTTP server for Raft RPCs. |

### Engine Submodules

| Module | File | Description |
|--------|------|-------------|
| `engine::mod` | `engine/mod.rs` | Core `Engine` struct. Manages V8 isolate creation, TypeScript transpilation (SWC), heap snapshot serialization, WASM module injection, concurrency limiting (semaphore), and the full execution lifecycle. |
| `engine::console` | `engine/console.rs` | Console output capture. Intercepts `console.log/info/warn/error` via a deno_core op and writes output as a byte stream into sled (WAL-style 4KB pages). |
| `engine::execution` | `engine/execution.rs` | Execution registry. Tracks in-flight and completed V8 executions in a `DashMap`, stores console output in per-execution sled trees, supports cancellation via `IsolateHandle::terminate_execution()`. |
| `engine::fetch` | `engine/fetch.rs` | OPA-gated `fetch()`. Implements a web-standard Fetch API via an async deno_core op. Requests are policy-checked before execution. Supports header injection rules. |
| `engine::fs` | `engine/fs.rs` | Policy-gated filesystem operations. Provides a Node.js-compatible `fs` API (readFile, writeFile, stat, mkdir, etc.) where every operation is evaluated against a PolicyChain. |
| `engine::heap_storage` | `engine/heap_storage.rs` | Heap snapshot persistence. Trait `HeapStorage` with implementations: `FileHeapStorage` (local disk), `S3HeapStorage` (AWS S3), `WriteThroughCacheHeapStorage` (S3 with local FS cache). |
| `engine::heap_tags` | `engine/heap_tags.rs` | Heap tag metadata store. Key-value tags on heap snapshots stored in sled. Supports cluster replication via Raft. |
| `engine::session_log` | `engine/session_log.rs` | Session logging. Records each execution's input/output heap hashes, code, and timestamps in sled. Supports cluster replication via Raft. |
| `engine::opa` | `engine/opa.rs` | OPA policy evaluation. Two backends: `RemotePolicyEvaluator` (OPA REST API) and `LocalPolicyEvaluator` (regorus for local Rego files). Composed into a `PolicyChain` with AND/OR modes. |
| `engine::module_loader` | `engine/module_loader.rs` | ES module loader. Resolves `npm:`, `jsr:`, and URL imports by rewriting them to esm.sh URLs. Supports policy-gated auditing and TypeScript transpilation of `.ts/.tsx` modules. |
| `engine::mcp_client` | `engine/mcp_client.rs` | MCP client manager. Connects to external MCP servers (via stdio or SSE) at startup and exposes their tools to JS code through `globalThis.mcp`. |

## Data Flow

### Stateless Execution

```mermaid
sequenceDiagram
    participant Client
    participant Transport as Transport<br/>(stdio/SSE/HTTP)
    participant MCP as StatelessMcpService
    participant Engine
    participant Registry as ExecutionRegistry
    participant V8 as V8 Isolate
    participant Console as Console (sled)

    Client->>Transport: run_js(code)
    Transport->>MCP: run_js(code)
    MCP->>Engine: run_js(code, None, None)
    Engine->>Registry: register(exec_id) → sled tree
    Engine->>Engine: strip TypeScript (SWC)
    Engine-->>V8: spawn on blocking thread
    V8->>Console: console.log → op_console_write → sled WAL
    V8-->>Engine: result / error
    Engine->>Registry: complete(exec_id, result)

    loop Poll until terminal
        MCP->>Registry: get(exec_id).status
    end

    MCP->>Registry: get_console_output(exec_id)
    MCP->>Client: {output, error?}
```

### Stateful Execution (with Heap Persistence)

```mermaid
sequenceDiagram
    participant Client
    participant MCP as McpService
    participant Engine
    participant Registry as ExecutionRegistry
    participant V8 as V8 Isolate<br/>(JsRuntimeForSnapshot)
    participant HeapStorage as Heap Storage<br/>(File/S3)
    participant SessionLog as Session Log
    participant HeapTags as Heap Tags

    Client->>MCP: run_js(code, heap?, session?, tags?)
    MCP->>Engine: run_js(code, heap, session, ...)
    Engine->>Registry: register(exec_id)

    alt heap provided
        Engine->>HeapStorage: get(heap_hash) → snapshot bytes
        Engine->>Engine: unwrap_snapshot (verify magic + SHA-256)
        Engine->>V8: create with startup_snapshot
    else session provided (no heap)
        Engine->>SessionLog: get_latest(session)
        Engine->>HeapStorage: get(latest_heap_hash)
        Engine->>V8: create with startup_snapshot
    else no heap / no session
        Engine->>V8: create fresh isolate
    end

    Engine->>Engine: strip TypeScript (SWC)
    V8->>V8: execute code
    Engine->>V8: take snapshot
    Engine->>Engine: wrap_snapshot (magic + SHA-256)
    Engine->>HeapStorage: put(new_hash, wrapped_snapshot)

    opt session provided
        Engine->>SessionLog: append(session, {input_heap, output_heap, code})
    end

    opt tags provided
        Engine->>HeapTags: merge_tags(new_hash, tags)
    end

    Engine->>Registry: complete(exec_id, result, new_heap_hash)
    MCP->>Client: {execution_id}
```

### Policy-Gated Fetch

```mermaid
sequenceDiagram
    participant JS as JavaScript Code
    participant FetchOp as op_fetch<br/>(deno_core async op)
    participant PolicyChain as PolicyChain
    participant Evaluator as Local Rego / Remote OPA
    participant HTTP as reqwest HTTP Client

    JS->>FetchOp: fetch("https://api.example.com")
    FetchOp->>FetchOp: Parse URL, build policy input
    FetchOp->>FetchOp: Apply header injection rules
    FetchOp->>PolicyChain: evaluate(input)

    alt mode = "all"
        loop Each evaluator
            PolicyChain->>Evaluator: evaluate_policy(input)
            Evaluator-->>PolicyChain: true/false
            Note over PolicyChain: Short-circuit on false
        end
    else mode = "any"
        loop Each evaluator
            PolicyChain->>Evaluator: evaluate_policy(input)
            Evaluator-->>PolicyChain: true/false
            Note over PolicyChain: Short-circuit on true
        end
    end

    alt Allowed
        FetchOp->>HTTP: send request
        HTTP-->>FetchOp: response
        FetchOp-->>JS: Response object
    else Denied
        FetchOp-->>JS: Error: "fetch denied by policy"
    end
```

## V8 Extension Architecture

Each V8 isolate is configured with deno_core extensions that bridge JavaScript to Rust:

```mermaid
graph TB
    subgraph "JavaScript Global Scope"
        console_js["console.log/info/warn/error"]
        fetch_js["fetch(url, opts)"]
        fs_js["fs.readFile/writeFile/stat/..."]
        mcp_js["mcp.callTool/listTools/servers"]
        import_js["import 'npm:...' / 'jsr:...'"]
    end

    subgraph "deno_core Extensions"
        console_ext["console_ext<br/>op_console_write"]
        fetch_ext["fetch_ext<br/>op_fetch"]
        fs_ext["fs_ext<br/>op_fs_read_file_text<br/>op_fs_write_file_text<br/>op_fs_stat<br/>...12 ops"]
        mcp_ext["mcp_client_ext<br/>op_mcp_call_tool<br/>op_mcp_list_tools<br/>op_mcp_list_servers"]
        mod_ext["NetworkModuleLoader<br/>(ModuleLoader trait)"]
    end

    subgraph "Rust Backends"
        sled_console["sled (WAL pages)"]
        reqwest_fetch["reqwest + OPA PolicyChain"]
        tokio_fs["tokio::fs + OPA PolicyChain"]
        rmcp_client["rmcp Peer + transport"]
        esm_sh["esm.sh CDN"]
    end

    console_js --> console_ext --> sled_console
    fetch_js --> fetch_ext --> reqwest_fetch
    fs_js --> fs_ext --> tokio_fs
    mcp_js --> mcp_ext --> rmcp_client
    import_js --> mod_ext --> esm_sh
```

## Storage Architecture

```mermaid
graph TB
    subgraph "Execution Data"
        ExecReg["ExecutionRegistry"]
        DashMap["DashMap<br/>(in-memory status)"]
        SledExec["sled trees<br/>ex:{id} (console WAL)"]
    end

    subgraph "Heap Persistence"
        HeapTrait["trait HeapStorage"]
        FileHS["FileHeapStorage<br/>(local directory)"]
        S3HS["S3HeapStorage<br/>(AWS S3 bucket)"]
        CacheHS["WriteThroughCacheHeapStorage<br/>(S3 + local FS cache)"]
    end

    subgraph "Session & Tag Metadata"
        SLog["SessionLog<br/>(sled DB)"]
        HTags["HeapTagStore<br/>(sled DB)"]
    end

    subgraph "Cluster Replication"
        Raft["Raft Consensus<br/>(ClusterNode)"]
        RaftLog["Raft Log<br/>(sled)"]
        DataTree["Replicated Data Tree<br/>(sled)"]
    end

    ExecReg --> DashMap
    ExecReg --> SledExec

    HeapTrait --> FileHS & S3HS & CacheHS
    CacheHS --> S3HS
    CacheHS --> FileHS

    SLog --> Raft
    HTags --> Raft
    Raft --> RaftLog
    Raft --> DataTree
```

## Cluster Architecture

The cluster layer implements a Raft-inspired consensus protocol for replicating session logs and heap tags across nodes.

```mermaid
graph TB
    subgraph "Node 1 (Leader)"
        N1_MCP["MCP Service"]
        N1_Engine["Engine"]
        N1_Raft["ClusterNode<br/>(Leader)"]
        N1_HTTP["Raft HTTP<br/>:9001"]
    end

    subgraph "Node 2 (Follower)"
        N2_MCP["MCP Service"]
        N2_Engine["Engine"]
        N2_Raft["ClusterNode<br/>(Follower)"]
        N2_HTTP["Raft HTTP<br/>:9002"]
    end

    subgraph "Node 3 (Follower)"
        N3_MCP["MCP Service"]
        N3_Engine["Engine"]
        N3_Raft["ClusterNode<br/>(Follower)"]
        N3_HTTP["Raft HTTP<br/>:9003"]
    end

    LB["Load Balancer<br/>(nginx)"]

    LB --> N1_MCP & N2_MCP & N3_MCP

    N1_HTTP <-->|AppendEntries<br/>RequestVote| N2_HTTP
    N1_HTTP <-->|AppendEntries<br/>RequestVote| N3_HTTP
    N2_HTTP <-->|AppendEntries<br/>RequestVote| N3_HTTP

    N2_Raft -->|Write forwarding| N1_Raft
    N3_Raft -->|Write forwarding| N1_Raft
```

Raft RPCs exposed on each node's cluster HTTP port:

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/raft/append_entries` | POST | AppendEntries RPC (log replication + heartbeat) |
| `/raft/request_vote` | POST | RequestVote RPC (leader election) |
| `/raft/join` | POST | Dynamic peer join |
| `/data/{key}` | GET | Read from replicated KV store |
| `/data/{key}` | PUT | Write to replicated KV store (forwarded to leader) |
| `/data/scan/{prefix}` | GET | Prefix scan on replicated KV store |
| `/raft/status` | GET | Node role, term, leader info |

## Transport Modes

The server supports three transport modes, selected at startup via CLI flags:

| Transport | Flag | Protocol | Load-Balanceable |
|-----------|------|----------|------------------|
| stdio | *(default)* | JSON-RPC over stdin/stdout | No |
| SSE | `--sse-port PORT` | HTTP+SSE (legacy MCP) | With sticky sessions |
| Streamable HTTP | `--http-port PORT` | Streamable HTTP (MCP 2025-03-26+) | Yes |

When using HTTP or SSE transports, the REST API router is merged into the same axum server, making both the MCP protocol and the REST API available on the same port.

## Key Design Decisions

- **deno_core for V8 embedding**: Provides the V8 runtime, async op system, module loading, and snapshot support without the full Deno runtime overhead.
- **SWC for TypeScript**: Type stripping only (no type checking) for minimal latency. Applied to both inline code and fetched `.ts/.tsx` modules.
- **sled for local storage**: Embedded key-value store used for execution console output (WAL-style), session logs, heap tags, and Raft log persistence.
- **Snapshot-based heap persistence**: V8 heap snapshots enable stateful sessions — the entire JS heap is serialized, stored, and restored across executions.
- **Snapshot envelope with SHA-256**: Prevents V8 `abort()` on corrupted snapshot data by validating a magic header + checksum before reaching V8's deserializer.
- **Bounded ArrayBuffer allocator**: Custom V8 allocator that tracks allocations and returns null on limit, converting OOM into a catchable JS `RangeError` instead of `abort()`.
- **Concurrency semaphore**: Limits concurrent V8 executions to prevent resource exhaustion (defaults to CPU core count).
- **Policy chains (OPA/Rego)**: fetch(), fs operations, and module imports are gated by composable policy chains supporting both local Rego files and remote OPA servers.
