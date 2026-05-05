use std::{env, fs, path::Path};

fn main() {
    // Embed the CLI binary if MCP_V8_CLI_BINARY is set.
    // If not set (local/dev builds), we write an empty file so the
    // include_bytes! in api.rs compiles without error; the /api/cli/{platform}
    // endpoint will return 404 with a message explaining the binary is not
    // embedded in this build.
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");

    for platform in &[
        "linux-x86_64",
        "linux-aarch64",
        "macos-x86_64",
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
