//! Embeds the icon and version info into the Windows executable.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/samplewright.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/samplewright.ico");
        res.set("ProductName", "SampleWright");
        res.set("FileDescription", "SampleWright: stems, loops and one-shots from any song");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the Windows icon: {e}");
        }
    }
}
