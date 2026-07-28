// プロジェクトのビルド設定およびアセットの埋め込み処理を行うビルドスクリプト
fn main() {
    println!("cargo:rerun-if-changed=app_icon.ico");
    // CARGO_CFG_TARGET_OS は「ビルド先のOS」を表します。
    // Wasmビルドの時は "unknown" や "none" になるため、Windows判定を回避できます。
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let mut res = winres::WindowsResource::new();

        // 元のアイコン指定を復活
        res.set_icon("app_icon.ico");

        // Wasm以外のエラーでパニックしないように unwrap() ではなく match で安全に処理
        match res.compile() {
            Err(e) => println!("cargo:warning=Failed to compile windows resources: {}", e),
            Ok(()) => {
                // `winres` links its resource to the library target. This
                // package also builds main.rs as an EXE, so link it explicitly.
                let resource = std::path::PathBuf::from(
                    std::env::var("OUT_DIR").expect("OUT_DIR is set by Cargo"),
                )
                .join("resource.lib");
                println!(
                    "cargo:rustc-link-arg-bin=Renjulibviewer_v2={}",
                    resource.display()
                );
            }
        }
    }
}
