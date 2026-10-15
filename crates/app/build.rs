fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let part = |name: &str| {
            format!(
                "PK_{name}={}",
                std::env::var(format!("CARGO_PKG_VERSION_{name}")).unwrap()
            )
        };
        let macros = [part("MAJOR"), part("MINOR"), part("PATCH")];
        embed_resource::compile("assets/app.rc", &macros)
            .manifest_optional()
            .unwrap();
    }
}
