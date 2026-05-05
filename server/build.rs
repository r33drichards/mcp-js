use std::{env, fs, path::Path, process::Command};

fn main() {
    // Override CARGO_PKG_VERSION with the git tag if RELEASE_VERSION is set
    // (injected by the release workflow as the tag name, e.g. "0.10.21").
    // Falls back to git describe, then to the Cargo.toml version.
    let version = env::var("RELEASE_VERSION")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| {
            // Try git describe --tags --exact-match (clean tag builds)
            Command::new("git")
                .args(["describe", "--tags", "--exact-match"])
                .output()
                .ok()
                .and_then(|o| {
                    if o.status.success() {
                        String::from_utf8(o.stdout).ok().map(|s| s.trim().trim_start_matches('v').to_string())
                    } else {
                        None
                    }
                })
        });

    if let Some(v) = version {
        // Cargo uses CARGO_PKG_VERSION from the env if set before the build
        // but the canonical way to override what env!("CARGO_PKG_VERSION") returns
        // is to emit a rustc-env instruction from build.rs.
        println!("cargo:rustc-env=CARGO_PKG_VERSION={}", v);
    }

    println!("cargo:rerun-if-env-changed=RELEASE_VERSION");

    // Embed the CLI binary if MCP_V8_CLI_BINARY is set.
    // If not set (local/dev builds), we write an empty file so the
    // include_bytes! in api.rs compiles without error; the /api/cli/{platform}
    // endpoint will return 404 with a message explaining the binary is not
    // embedded in this build.
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");

    for platform in &[
        "linux-x86_64",
        "linux-aarch64",
        "macos-aarch64",
    ] {
        let env_key = format!(
            "MCP_V8_CLI_{}",
            platform.to_uppercase().replace('-', "_")
        );
        let out_path = Path::new(&out_dir).join(format!("cli-{}.bin", platform));

        if let Ok(src) = env::var(&env_key) {
            println!("cargo:rerun-if-env-changed={}", env_key);
            println!("cargo:rerun-if-changed={}", src);
            fs::copy(&src, &out_path)
                .unwrap_or_else(|e| panic!("Failed to copy CLI binary from {}: {}", src, e));
        } else {
            // Write empty file so include_bytes! still compiles
            fs::write(&out_path, b"")
                .unwrap_or_else(|e| panic!("Failed to write empty CLI placeholder: {}", e));
        }
        println!("cargo:rerun-if-env-changed={}", env_key);
    }
}
