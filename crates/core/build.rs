fn main() {
    // the unrar library calls the registry and security APIs and leaves them for the final link to resolve
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        println!("cargo:rustc-link-lib=advapi32");
    }
}
