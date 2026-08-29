fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android")
        && std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64")
    {
        println!("cargo:rustc-link-arg=-Wl,-z,max-page-size=16384");
    }
}
