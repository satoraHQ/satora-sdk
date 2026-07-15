fn main() {
    // Modern uniffi (proc-macro-driven) doesn't need a UDL file — every
    // exported item carries its own scaffolding. This build script is
    // kept around for visibility / future use (codegen toggles,
    // platform-specific link flags, etc.).
    println!("cargo:rerun-if-changed=src/lib.rs");
}
