//! Compiles the admin gRPC proto into client and server stubs in `OUT_DIR`.

fn main() {
    tonic_build::configure()
        .build_client(true)
        .build_server(true)
        .compile_protos(&["proto/admin.proto"], &["proto"])
        .expect("compile proto/admin.proto");
    println!("cargo:rerun-if-changed=proto/admin.proto");
}
