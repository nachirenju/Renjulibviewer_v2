// プロジェクトのビルド設定およびアセットの埋め込み処理を行うビルドスクリプト
fn main() {
    // CARGO_CFG_TARGET_OS は「ビルド先のOS」を表します。
    // Wasmビルドの時は "unknown" や "none" になるため、Windows判定を回避できます。
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let mut res = winres::WindowsResource::new();
        
        // 元のアイコン指定を復活
        res.set_icon("app_icon.ico");
        
        // Wasm以外のエラーでパニックしないように unwrap() ではなく match で安全に処理
        if let Err(e) = res.compile() {
            println!("cargo:warning=Failed to compile windows resources: {}", e);
        }
    }
}