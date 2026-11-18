fn main() {
    // Emits rpath and stages the zvec shared library beside the binary,
    // mirroring the `zg` binary's build script.
    zvec_rust_build::configure();
}
