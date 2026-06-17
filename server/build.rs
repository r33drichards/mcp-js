use std::{env, fs, path::Path, process::Command};

fn main() {
                let version = env::var("RELEASE_VERSION")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| {
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
                                println!("cargo:rustc-env=CARGO_PKG_VERSION={}", v);
    }

    println!("cargo:rerun-if-env-changed=RELEASE_VERSION");

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
                        fs::write(&out_path, b"")
                .unwrap_or_else(|e| panic!("Failed to write empty CLI placeholder: {}", e));
        }
        println!("cargo:rerun-if-env-changed={}", env_key);
    }
}
