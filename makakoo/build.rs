fn main() {
    // Windows' default 1 MiB main-thread stack is too small for the debug
    // makakoo CLI under clap/tokio in CI; adapter integration tests spawned
    // makakoo.exe and overflowed before printing diagnostics. Reserve 8 MiB
    // for the shipped binary as well so Windows users get the same headroom.
    if std::env::var("CARGO_CFG_WINDOWS").is_ok() {
        println!("cargo:rustc-link-arg-bin=makakoo=/STACK:8388608");
    }
}
