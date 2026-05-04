fn main() {
    let spec_path = std::env::var("OPENAPI_SPEC_PATH")
        .unwrap_or_else(|_| "openapi.json".to_string());

    let file = std::fs::File::open(&spec_path)
        .unwrap_or_else(|e| panic!("Failed to open OpenAPI spec at {}: {}", spec_path, e));

    let spec = serde_json::from_reader(file)
        .expect("Failed to parse OpenAPI spec");

    let mut generator = progenitor::Generator::default();

    let tokens = generator.generate_tokens(&spec)
        .expect("Failed to generate client code");

    let ast = syn::parse2(tokens)
        .expect("Failed to parse generated code");

    let content = prettyplease::unparse(&ast);

    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest_path = std::path::Path::new(&out_dir).join("codegen.rs");

    std::fs::write(&dest_path, content)
        .expect("Failed to write generated code");

    println!("cargo:rerun-if-changed={}", spec_path);
}
