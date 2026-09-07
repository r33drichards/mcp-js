// Example capability-bearing JS hook: audit every filesystem write to a
// log file, without gating anything. Wire up with:
//
//   --policies-json '{
//     "filesystem": {
//       "pre": [{"url": "file:///etc/policies/audit_fs_hooks.js",
//                "capabilities": ["fs"]}]
//     }
//   }'
//
// The "capabilities" list grants this hook's isolate pieces of the guest
// environment, with the same JS API the sandbox sees: "fs" installs the
// fs.* wrapper, "fetch" installs fetch() (plus atob/btoa). Hook-issued
// operations are UNGATED — they run through no hook chain and no policy —
// both because this file is operator-trusted config (the same trust as the
// policy files themselves) and because gating them would recurse into the
// very chain the hook runs inside.
//
// Capability-bearing hooks may be async; the call is still bounded by
// timeout_ms (default 5000 ms) and fails the operation on expiry.

const LOG = "/var/log/mcp-js/fs-audit.log";
const WRITE_OPS = new Set([
    "writeFile", "appendFile", "mkdir", "remove", "rename", "copyFile",
    "symlink", "truncate", "chmod", "utimes",
]);

async function pre(input) {
    if (WRITE_OPS.has(input.operation)) {
        const dest = input.destination ? " -> " + input.destination : "";
        await fs.appendFile(
            LOG,
            new Date().toISOString() + " " + input.operation + " " + input.path + dest + "\n"
        );
    }
    // No return value: observe and abstain — the operation proceeds
    // unchanged to the rest of the chain (and the policy, if configured).
}
