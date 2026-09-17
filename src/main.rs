#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
#[cfg(target_arch = "wasm32")]
use serde::{Deserialize, Serialize};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::Ordering;
#[cfg(target_arch = "wasm32")]
use std::sync::{Arc, Mutex};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::closure::Closure;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

mod board;
mod core;
mod exportkifu;
mod io;
mod lang;
mod setting;
mod ui;
mod vcf;

use crate::core::hash_board::Board as HashBoard;
use crate::core::node::{NO_NODE, NO_TEXT, RenLibNode, TextPool};
use crate::io::parser::RenLibParser;
use crate::io::writer::RenLibWriter;
use crate::vcf::model::BOARD_CELLS;
use crate::vcf::solver::{BLACK, Move as VcfMove, SearchMode, SolveOutcome, Solver, side_to_move};
use crate::vcf::vcf_threat::{ThreatSearchOutcome, find_vcf_threats};
use board::Board;
use rustc_hash::FxHashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::Receiver;

#[allow(dead_code)] // Used by the wasm worker path; the library target has no entry point.
#[allow(dead_code)] // Used by the wasm worker path; the library target has no entry point.
fn search_mode_wire(mode: SearchMode) -> &'static str {
    match mode {
        SearchMode::Long => "long",
        SearchMode::FirstWin => "first_win",
        SearchMode::Fast => "fast",
        SearchMode::Shortest => "shortest",
    }
}

fn search_mode_from_setting_for_vcf(mode: setting::VcfMode) -> SearchMode {
    match mode {
        setting::VcfMode::Long => SearchMode::Long,
        setting::VcfMode::FirstWin | setting::VcfMode::Shortest => SearchMode::Shortest,
    }
}

#[cfg(target_os = "android")]
mod android_file {
    use android_activity::AndroidApp;
    use jni::{
        JavaVM, jni_sig, jni_str,
        objects::{JObject, JString, JValue},
        refs::Global,
    };
    use std::{
        ffi::c_void,
        fs::File,
        os::fd::FromRawFd,
        sync::atomic::{AtomicPtr, Ordering},
    };

    static ACTIVITY: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

    #[derive(Debug)]
    pub struct OpenedFile {
        pub file: File,
        pub file_name: String,
    }

    pub fn init(app: &AndroidApp) {
        ACTIVITY.store(app.activity_as_ptr(), Ordering::Release);
    }

    pub fn clear() {
        ACTIVITY.store(std::ptr::null_mut(), Ordering::Release);
    }

    fn with_activity<T>(
        f: impl for<'local> FnOnce(&mut jni::Env<'local>, &JObject<'local>) -> jni::errors::Result<T>,
    ) -> Result<T, String> {
        let vm = JavaVM::singleton().map_err(|err| format!("{err:?}"))?;
        let activity_raw = ACTIVITY.load(Ordering::Acquire) as jni::sys::jobject;
        if activity_raw.is_null() {
            return Err("Android activity is not initialized".to_string());
        }
        vm.attach_current_thread(|env| {
            // AndroidApp owns this global JNI reference. Borrow it without
            // constructing an owned local/global wrapper, as required by
            // android-activity's activity_as_ptr contract.
            let activity = {
                let activity_global = unsafe { env.as_cast_raw::<Global<JObject>>(&activity_raw)? };
                env.new_local_ref(activity_global.as_ref())?
            };
            f(env, &activity)
        })
        .map_err(|err| format!("{err:?}"))
    }

    fn call_take_string(method: TakeStringMethod) -> Result<Option<String>, String> {
        with_activity(|env, activity| {
            let value = match method {
                TakeStringMethod::OpenedFileName => env
                    .call_method(
                        activity,
                        jni_str!("takeOpenedFileName"),
                        jni_sig!(() -> JString),
                        &[],
                    )?
                    .l()?,
                TakeStringMethod::FileError => env
                    .call_method(
                        activity,
                        jni_str!("takeFileError"),
                        jni_sig!(() -> JString),
                        &[],
                    )?
                    .l()?,
                TakeStringMethod::FontPath => env
                    .call_method(
                        activity,
                        jni_str!("takeFontPath"),
                        jni_sig!(() -> JString),
                        &[],
                    )?
                    .l()?,
                TakeStringMethod::FontName => env
                    .call_method(
                        activity,
                        jni_str!("takeFontName"),
                        jni_sig!(() -> JString),
                        &[],
                    )?
                    .l()?,
            };
            if value.as_raw().is_null() {
                return Ok(None);
            }
            let jstr = env.cast_local::<JString>(value)?;
            Ok(Some(jstr.to_string()))
        })
    }

    enum TakeStringMethod {
        OpenedFileName,
        FileError,
        FontPath,
        FontName,
    }

    fn take_opened_fd() -> Result<Option<File>, String> {
        take_fd(true)
    }

    fn take_save_fd() -> Result<Option<File>, String> {
        take_fd(false)
    }

    fn take_fd(opened: bool) -> Result<Option<File>, String> {
        let fd = with_activity(|env, activity| {
            if opened {
                env.call_method(activity, jni_str!("takeOpenedFd"), jni_sig!(() -> int), &[])?
                    .i()
            } else {
                env.call_method(activity, jni_str!("takeSaveFd"), jni_sig!(() -> int), &[])?
                    .i()
            }
        })?;
        if fd < 0 {
            Ok(None)
        } else {
            let file = unsafe { File::from_raw_fd(fd) };
            Ok(Some(file))
        }
    }

    pub fn request_open() -> Result<(), String> {
        with_activity(|env, activity| {
            env.call_method(
                activity,
                jni_str!("openRenjuFile"),
                jni_sig!(() -> void),
                &[],
            )?;
            Ok(())
        })
    }

    pub fn request_save(file_name: &str) -> Result<(), String> {
        with_activity(|env, activity| {
            let name = env.new_string(file_name)?;
            env.call_method(
                activity,
                jni_str!("saveRenjuFile"),
                jni_sig!((name: JString) -> void),
                &[JValue::Object(&name)],
            )?;
            Ok(())
        })
    }

    pub fn request_open_font() -> Result<(), String> {
        with_activity(|env, activity| {
            env.call_method(
                activity,
                jni_str!("openFontFile"),
                jni_sig!(() -> void),
                &[],
            )?;
            Ok(())
        })
    }

    pub fn settings_path() -> Result<std::path::PathBuf, String> {
        with_activity(|env, activity| {
            let value = env
                .call_method(
                    activity,
                    jni_str!("getSettingsPath"),
                    jni_sig!(() -> JString),
                    &[],
                )?
                .l()?;
            let jstr = env.cast_local::<JString>(value)?;
            Ok(std::path::PathBuf::from(jstr.to_string()))
        })
    }

    pub fn take_font_selection() -> Result<Option<(String, String)>, String> {
        let path = match call_take_string(TakeStringMethod::FontPath)? {
            Some(path) => path,
            None => return Ok(None),
        };
        let name = call_take_string(TakeStringMethod::FontName)?
            .unwrap_or_else(|| "Selected font".to_string());
        Ok(Some((path, name)))
    }

    pub fn set_clipboard_text(text: &str) -> Result<(), String> {
        with_activity(|env, activity| {
            let text = env.new_string(text)?;
            env.call_method(
                activity,
                jni_str!("setClipboardText"),
                jni_sig!((text: JString) -> void),
                &[JValue::Object(&text)],
            )?;
            Ok(())
        })
    }

    pub fn get_clipboard_text() -> Result<Option<String>, String> {
        with_activity(|env, activity| {
            let value = env
                .call_method(
                    activity,
                    jni_str!("getClipboardText"),
                    jni_sig!(() -> JString),
                    &[],
                )?
                .l()?;
            if value.as_raw().is_null() {
                return Ok(None);
            }
            let jstr = env.cast_local::<JString>(value)?;
            Ok(Some(jstr.to_string()))
        })
    }

    pub fn safe_inset_top() -> Result<i32, String> {
        with_activity(|env, activity| {
            env.call_method(
                activity,
                jni_str!("getSafeInsetTop"),
                jni_sig!(() -> int),
                &[],
            )?
            .i()
        })
    }

    pub fn take_opened_file() -> Result<Option<OpenedFile>, String> {
        let file = match take_opened_fd()? {
            Some(file) => file,
            None => return Ok(None),
        };
        let file_name = call_take_string(TakeStringMethod::OpenedFileName)?
            .unwrap_or_else(|| "selected.lib".to_string());
        Ok(Some(OpenedFile { file, file_name }))
    }

    pub fn take_save_file() -> Result<Option<File>, String> {
        take_save_fd()
    }

    pub fn take_error() -> Result<Option<String>, String> {
        call_take_string(TakeStringMethod::FileError)
    }
}

#[cfg(target_os = "ios")]
mod ios_file {
    pub fn request_open() -> Result<(), String> {
        Err("iOS document picker is not implemented yet".to_string())
    }

    pub fn request_save(_file_name: &str) -> Result<(), String> {
        Err("iOS document save picker is not implemented yet".to_string())
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(inline_js = r#"
export function isVcfWorkerContext() {
    return globalThis.__RENJU_VCF_WORKER === true;
}

export function postVcfWorkerProgress(message) {
    if (globalThis.__RENJU_VCF_WORKER === true && typeof self !== "undefined") {
        self.postMessage(message);
    }
}

function findTrunkAsset(selector, fallbackPattern, predicate = () => true) {
    for (const element of Array.from(document.querySelectorAll(selector))) {
        if (element.href && predicate(element.href)) {
            return element.href;
        }
    }

    for (const script of Array.from(document.scripts)) {
        const text = script.textContent || "";
        const match = text.match(fallbackPattern);
        if (match && match[1]) {
            return new URL(match[1], document.baseURI).href;
        }
    }

    return null;
}

export function spawnVcfWorker(requestJson, callback) {
    const moduleUrl = findTrunkAsset(
        'link[rel="modulepreload"][href$=".js"]',
        /import\s+init,\s*\*\s*as\s+bindings\s+from\s+['"]([^'"]+\.js)['"]/,
        (href) => href.includes("Renjulibviewer_v2") && !href.includes("/snippets/")
    );
    const wasmUrl = findTrunkAsset(
        'link[rel="preload"][as="fetch"][href$=".wasm"]',
        /module_or_path:\s*['"]([^'"]+\.wasm)['"]/
    );

    if (!moduleUrl || !wasmUrl) {
        throw new Error("VCF worker could not find Trunk wasm assets");
    }

    const workerSource = `
        globalThis.__RENJU_VCF_WORKER = true;
        let wasmReady = null;

        async function ensureWasm() {
            if (!wasmReady) {
                const module = await import(${JSON.stringify(moduleUrl)});
                wasmReady = module.default({ module_or_path: ${JSON.stringify(wasmUrl)} })
                    .then(() => module);
            }
            return await wasmReady;
        }

        self.onmessage = async (event) => {
            try {
                const module = await ensureWasm();
                const result = module.vcf_solve_worker(event.data);
                self.postMessage(result);
            } catch (error) {
                const message = error && error.stack ? error.stack : String(error);
                self.postMessage(JSON.stringify({ kind: "Error", message }));
            }
        };
    `;

    const url = URL.createObjectURL(new Blob([workerSource], { type: "text/javascript" }));
    const worker = new Worker(url, { type: "module" });
    URL.revokeObjectURL(url);

    worker.onmessage = (event) => {
        callback(event.data);
        let shouldTerminate = true;
        try {
            const parsed = JSON.parse(event.data);
            if (parsed && parsed.kind === "Progress") {
                return;
            }
        } catch (_) {}
        if (shouldTerminate) {
            worker.terminate();
        }
    };
    worker.onerror = (event) => {
        callback(JSON.stringify({ kind: "Error", message: event.message || "VCF worker error" }));
        worker.terminate();
    };
    worker.postMessage(requestJson);
    return worker;
}
"#)]
extern "C" {
    #[wasm_bindgen(js_name = isVcfWorkerContext)]
    fn is_vcf_worker_context() -> bool;
    #[wasm_bindgen(js_name = postVcfWorkerProgress)]
    fn post_vcf_worker_progress(message: &str);

    #[wasm_bindgen(catch, js_name = spawnVcfWorker)]
    fn spawn_vcf_worker(
        request_json: &str,
        callback: &js_sys::Function,
    ) -> Result<web_sys::Worker, JsValue>;
}

#[cfg(target_arch = "wasm32")]
#[derive(Serialize, Deserialize)]
struct VcfWorkerRequest {
    job: String,
    board: Vec<u8>,
    turn: u8,
    depth: usize,
    timeout_secs: u64,
    mode: String,
}

#[cfg(target_arch = "wasm32")]
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind")]
enum VcfWorkerResponse {
    Found { moves: Vec<VcfMove> },
    VctFound { moves: Vec<VcfMove> },
    TacticalVctFound { moves: Vec<VcfMove> },
    Threats { moves: Vec<VcfMove> },
    Progress { moves: Vec<VcfMove> },
    NotFound,
    Incomplete,
    Timeout,
    Cancelled,
    Error { message: String },
}

#[cfg(not(target_arch = "wasm32"))]
pub enum VcfJobOutcome {
    Solve(SolveOutcome),
    Vct(SolveOutcome),
    Threats(ThreatSearchOutcome),
}

#[cfg(target_arch = "wasm32")]
impl From<SolveOutcome> for VcfWorkerResponse {
    fn from(outcome: SolveOutcome) -> Self {
        match outcome {
            SolveOutcome::Found(moves) => Self::Found { moves },
            SolveOutcome::TacticalFound(moves) => Self::Found { moves },
            SolveOutcome::NotFound => Self::NotFound,
            SolveOutcome::Incomplete => Self::Incomplete,
            SolveOutcome::Timeout => Self::Timeout,
            SolveOutcome::Cancelled => Self::Cancelled,
        }
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn vcf_solve_worker(request_json: &str) -> String {
    let response = match serde_json::from_str::<VcfWorkerRequest>(request_json) {
        Ok(request) => {
            let board: [u8; BOARD_CELLS] = match request.board.try_into() {
                Ok(board) => board,
                Err(board) => {
                    return serde_json::to_string(&VcfWorkerResponse::Error {
                        message: format!(
                            "Invalid VCF worker board length: expected {BOARD_CELLS}, got {}",
                            board.len()
                        ),
                    })
                    .unwrap_or_else(|_| {
                        "{\"kind\":\"Error\",\"message\":\"Invalid VCF worker board\"}".to_string()
                    });
                }
            };
            let mode = match request.mode.as_str() {
                "long" => SearchMode::Long,
                "first_win" => SearchMode::FirstWin,
                "fast" => SearchMode::Fast,
                _ => SearchMode::Shortest,
            };
            let timeout = std::time::Duration::from_secs(request.timeout_secs.max(1));
            if request.job == "threats" {
                match find_vcf_threats(board, request.turn, request.depth, timeout, mode, None) {
                    ThreatSearchOutcome::Complete(moves) | ThreatSearchOutcome::Partial(moves) => {
                        VcfWorkerResponse::Threats { moves }
                    }
                    ThreatSearchOutcome::Cancelled => VcfWorkerResponse::Cancelled,
                }
            } else if request.job == "vct" {
                match Solver::solve_vct_with_progress(
                    board,
                    request.turn,
                    request.depth,
                    timeout,
                    Arc::new(AtomicBool::new(false)),
                    mode,
                    |moves| {
                        if let Ok(json) =
                            serde_json::to_string(&VcfWorkerResponse::Progress { moves })
                        {
                            post_vcf_worker_progress(&json);
                        }
                    },
                ) {
                    SolveOutcome::Found(moves) => VcfWorkerResponse::VctFound { moves },
                    SolveOutcome::TacticalFound(moves) => {
                        VcfWorkerResponse::TacticalVctFound { moves }
                    }
                    SolveOutcome::NotFound => VcfWorkerResponse::NotFound,
                    SolveOutcome::Incomplete => VcfWorkerResponse::Incomplete,
                    SolveOutcome::Timeout => VcfWorkerResponse::Timeout,
                    SolveOutcome::Cancelled => VcfWorkerResponse::Cancelled,
                }
            } else {
                VcfWorkerResponse::from(Solver::solve(
                    board,
                    request.turn,
                    request.depth,
                    timeout,
                    mode,
                ))
            }
        }
        Err(error) => VcfWorkerResponse::Error {
            message: format!("Invalid VCF worker request: {error}"),
        },
    };

    serde_json::to_string(&response).unwrap_or_else(|error| {
        format!(
            "{{\"kind\":\"Error\",\"message\":\"Failed to encode VCF worker response: {error}\"}}"
        )
    })
}

// ==========================================
// 繝輔か繝ｳ繝郁ｨｭ螳・// ==========================================

/// 繧ｫ繧ｹ繧ｿ繝繝輔か繝ｳ繝医ｒ隱ｭ縺ｿ霎ｼ縺ｿ縲‘gui繧ｳ繝ｳ繝・く繧ｹ繝医↓逋ｻ骭ｲ縺吶ｋ
pub fn setup_custom_fonts(ctx: &egui::Context, _path: &str) {
    let mut fonts = egui::FontDefinitions::default();

    #[cfg(all(
        not(target_arch = "wasm32"),
        not(target_os = "android"),
        not(target_os = "ios")
    ))]
    {
        if let Ok(font_data) = std::fs::read(_path) {
            fonts.font_data.insert(
                "custom_font".to_owned(),
                Arc::new(egui::FontData::from_owned(font_data)),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "custom_font".to_owned());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .insert(0, "custom_font".to_owned());
            ctx.set_fonts(fonts);
        }
    }

    #[cfg(target_os = "android")]
    {
        let builtin = match _path {
            "builtin:sc" => Some((
                "font_sc",
                include_bytes!("../Renjulibviewer_SC.ttf").as_slice(),
            )),
            "builtin:tc" => Some((
                "font_tc",
                include_bytes!("../Renjulibviewer_TC.ttf").as_slice(),
            )),
            "builtin:kr" => Some((
                "font_kr",
                include_bytes!("../Renjulibviewer_KR.ttf").as_slice(),
            )),
            "builtin:jp" => Some((
                "font_jp",
                include_bytes!("../Renjulibviewer_JP.ttf").as_slice(),
            )),
            _ => None,
        };

        let font_name = "android_selected_font".to_owned();
        let font_data = builtin
            .map(|(_, bytes)| egui::FontData::from_static(bytes))
            .or_else(|| std::fs::read(_path).ok().map(egui::FontData::from_owned))
            .unwrap_or_else(|| {
                egui::FontData::from_static(include_bytes!("../Renjulibviewer_JP.ttf"))
            });
        fonts
            .font_data
            .insert(font_name.clone(), Arc::new(font_data));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .insert(0, font_name.clone());
        }
        ctx.set_fonts(fonts);
    }

    #[cfg(target_os = "ios")]
    {
        let font_jp = include_bytes!("../Renjulibviewer_JP.ttf");
        let font_sc = include_bytes!("../Renjulibviewer_SC.ttf");
        let font_tc = include_bytes!("../Renjulibviewer_TC.ttf");
        let font_kr = include_bytes!("../Renjulibviewer_KR.ttf");

        fonts.font_data.insert(
            "font_jp".to_owned(),
            Arc::new(egui::FontData::from_static(font_jp)),
        );
        fonts.font_data.insert(
            "font_sc".to_owned(),
            Arc::new(egui::FontData::from_static(font_sc)),
        );
        fonts.font_data.insert(
            "font_tc".to_owned(),
            Arc::new(egui::FontData::from_static(font_tc)),
        );
        fonts.font_data.insert(
            "font_kr".to_owned(),
            Arc::new(egui::FontData::from_static(font_kr)),
        );

        let families = [egui::FontFamily::Proportional, egui::FontFamily::Monospace];

        for family in families {
            let entry = fonts.families.entry(family).or_default();
            entry.insert(0, "font_kr".to_owned());
            entry.insert(0, "font_tc".to_owned());
            entry.insert(0, "font_sc".to_owned());
            entry.insert(0, "font_jp".to_owned());
        }

        ctx.set_fonts(fonts);
    }

    #[cfg(target_arch = "wasm32")]
    {
        // 蜷・ヵ繧ｩ繝ｳ繝医ョ繝ｼ繧ｿ縺ｮ隱ｭ縺ｿ霎ｼ縺ｿ
        let font_jp = include_bytes!("../Renjulibviewer_JP.ttf");
        let font_sc = include_bytes!("../Renjulibviewer_SC.ttf");
        let font_tc = include_bytes!("../Renjulibviewer_TC.ttf");
        let font_kr = include_bytes!("../Renjulibviewer_KR.ttf");

        fonts.font_data.insert(
            "font_jp".to_owned(),
            Arc::new(egui::FontData::from_static(font_jp)),
        );
        fonts.font_data.insert(
            "font_sc".to_owned(),
            Arc::new(egui::FontData::from_static(font_sc)),
        );
        fonts.font_data.insert(
            "font_tc".to_owned(),
            Arc::new(egui::FontData::from_static(font_tc)),
        );
        fonts.font_data.insert(
            "font_kr".to_owned(),
            Arc::new(egui::FontData::from_static(font_kr)),
        );

        // 蜆ｪ蜈磯・ｽ阪・險ｭ螳・ JP -> SC -> TC -> KR
        let families = [egui::FontFamily::Proportional, egui::FontFamily::Monospace];

        for family in families {
            let vec = fonts.families.entry(family).or_default();
            vec.insert(0, "font_jp".to_owned());
            vec.push("font_sc".to_owned());
            vec.push("font_tc".to_owned());
            vec.push("font_kr".to_owned());
        }

        ctx.set_fonts(fonts);
    }
}

/// 繧｢繝励Μ繧ｱ繝ｼ繧ｷ繝ｧ繝ｳ繧｢繧､繧ｳ繝ｳ繧定ｪｭ縺ｿ霎ｼ繧
#[cfg(all(
    not(target_arch = "wasm32"),
    not(target_os = "android"),
    not(target_os = "ios")
))]
#[allow(dead_code)] // Used by the native binary entry point, not by the cdylib target.
#[allow(dead_code)] // Used by the native binary entry point, not by the cdylib target.
fn load_icon() -> egui::IconData {
    let icon_data = include_bytes!("../app_icon.png");
    let image = image::load_from_memory(icon_data)
        .expect("Failed to load icon")
        .into_rgba8();

    let (width, height) = image.dimensions();
    let rgba = image.into_raw();

    egui::IconData {
        rgba,
        width,
        height,
    }
}

// ==========================================
// 繧ｨ繝ｳ繝医Μ繝昴う繝ｳ繝・(襍ｷ蜍募・逅・
// ==========================================

/// Native entry point.
#[cfg(all(
    not(target_arch = "wasm32"),
    not(target_os = "android"),
    not(target_os = "ios")
))]
#[allow(dead_code)] // The same source also builds the cdylib target.
#[allow(dead_code)] // The same source also builds the cdylib target.
fn main() -> eframe::Result<()> {
    let mut args = std::env::args().skip(1);
    let maybe_path = args.next();

    let icon = load_icon();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_icon(Arc::new(icon)),
        ..Default::default()
    };

    eframe::run_native(
        "Renjulibviewer",
        options,
        Box::new(|cc| {
            let settings = setting::Settings::load();
            setup_custom_fonts(&cc.egui_ctx, &settings.font_path);

            // 繧｢繝励Μ譛ｬ菴薙ｒ逕滓・
            let mut app = RenjuApp::new_with_settings(settings);

            // 襍ｷ蜍墓凾縺ｫ繝輔ぃ繧､繝ｫ縺梧ｸ｡縺輔ｌ縺ｦ縺・ｌ縺ｰ蜊ｳ隱ｭ霎ｼ
            if let Some(path_str) = maybe_path {
                app.load_file(std::path::Path::new(&path_str));
            }

            Ok(Box::new(app))
        }),
    )
}

/// Android entry point.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub fn android_main(app: android_activity::AndroidApp) {
    android_file::init(&app);

    let options = eframe::NativeOptions {
        android_app: Some(app),
        ..Default::default()
    };

    let result = eframe::run_native(
        "Renjulibviewer",
        options,
        Box::new(|cc| {
            let settings = setting::Settings::load();
            setup_custom_fonts(&cc.egui_ctx, &settings.font_path);
            Ok(Box::new(RenjuApp::new_with_settings(settings)))
        }),
    );
    android_file::clear();

    if let Err(err) = result {
        eprintln!("Android eframe startup/runtime failure: {err}");
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {
    if is_vcf_worker_context() {
        return;
    }

    eframe::WebLogger::init(log::LevelFilter::Warn).ok();
    let web_options = eframe::WebOptions::default();

    wasm_bindgen_futures::spawn_local(async {
        let document = web_sys::window().unwrap().document().unwrap();
        let canvas = document.get_element_by_id("the_canvas_id").unwrap();
        let canvas: web_sys::HtmlCanvasElement = canvas
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .map_err(|_| ())
            .unwrap();

        eframe::WebRunner::new()
            .start(
                canvas,
                web_options,
                Box::new(|cc| {
                    let settings = setting::Settings::load();
                    setup_custom_fonts(&cc.egui_ctx, &settings.font_path);

                    let mut style = (*cc.egui_ctx.style()).clone();
                    style.text_styles = [
                        (
                            egui::TextStyle::Body,
                            egui::FontId::new(16.0, egui::FontFamily::Proportional),
                        ),
                        (
                            egui::TextStyle::Button,
                            egui::FontId::new(16.0, egui::FontFamily::Proportional),
                        ),
                        (
                            egui::TextStyle::Monospace,
                            egui::FontId::new(16.0, egui::FontFamily::Monospace),
                        ),
                        (
                            egui::TextStyle::Heading,
                            egui::FontId::new(18.0, egui::FontFamily::Proportional),
                        ),
                    ]
                    .into();
                    cc.egui_ctx.set_style(style);

                    // 繝悶Λ繧ｦ繧ｶ蛛ｴ縺ｫ縲悟香蛻・↑螟ｧ縺阪＆縲阪→隱崎ｭ倥＆縺帙ｋ縺溘ａ繧ｺ繝ｼ繝繝輔ぃ繧ｯ繧ｿ繧定ｪｿ謨ｴ
                    cc.egui_ctx.set_zoom_factor(1.1);

                    Ok(Box::new(RenjuApp::new_with_settings(settings)))
                }),
            )
            .await
            .expect("failed to start eframe");
    });
}

// ==========================================
// 繧｢繝励Μ繧ｱ繝ｼ繧ｷ繝ｧ繝ｳ縺ｮ迥ｶ諷狗ｮ｡逅・→繝｡繧､繝ｳ繝ｭ繧ｸ繝・け
// ==========================================

#[derive(Clone, PartialEq)]
pub struct GifSettings {
    start_move: usize,
    end_move: usize,
    show_number_start_move: usize,
    show_number_start_value: usize,
    frame_delay_sec: f32,
    first_frame_delay_sec: f32,
    last_frame_delay_sec: f32,
}

impl Default for GifSettings {
    fn default() -> Self {
        Self {
            start_move: 1,
            end_move: 225,
            show_number_start_move: 1,
            show_number_start_value: 1,
            frame_delay_sec: 1.0,
            first_frame_delay_sec: 2.0,
            last_frame_delay_sec: 5.0,
        }
    }
}

// ExportState 螳夂ｾｩ縺ｯ縺昴・縺ｾ縺ｾ
#[derive(Clone, PartialEq)]
pub enum ExportState {
    Idle,
    RequestFull,
    SelectingRegion(Option<egui::Pos2>, Option<egui::Rect>),
    WaitCapture(egui::Rect),
    CaptureRequested(egui::Rect),
    GifRequestFull,
    GifSelectingRegion(Option<egui::Pos2>, Option<egui::Rect>),
    GifWaitCapture(egui::Rect, usize),
    GifCaptureRequested(egui::Rect, usize),
}

pub struct RenjuApp {
    pub board: Board,
    pub lib_nodes: Option<Vec<RenLibNode>>,
    pub hash_table: Option<FxHashMap<u64, u32>>,
    pub node_texts: Option<Vec<u32>>,
    pub text_pool: Option<TextPool>,
    pub current_node_idx: Option<usize>,
    pub is_db_mode: bool,
    pub notation_text: String,
    pub sgf_text: String,
    pub portal_v1_text: String,
    pub portal_v2_text: String,
    pub settings: setting::Settings,
    pub node_to_delete: Option<usize>,
    pub editing_coord: Option<(i8, i8)>,
    pub editing_text: String,
    pub ad_hoc_labels: FxHashMap<(i8, i8), String>,
    pub current_file: Option<String>,
    pub current_filepath: Option<std::path::PathBuf>,
    pub load_time_sec: Option<f64>,
    pub subtree_cache: std::cell::RefCell<FxHashMap<usize, usize>>,
    pub is_text_mode: bool,
    pub export_state: ExportState,
    pub gif_settings: GifSettings,
    pub show_gif_setup: bool,
    pub gif_frames: Vec<image::RgbaImage>,
    pub show_about: bool,
    pub show_new_game_confirm: bool,
    #[cfg(target_os = "windows")]
    pub show_vct_log: bool,
    pub copy_notification: Option<(String, web_time::Instant)>,
    pub vcf_solution: Vec<VcfMove>,
    pub vcf_replay_len: usize,
    pub vcf_threats: Vec<VcfMove>,
    pub vcf_status: String,
    pub vcf_solving: bool,
    pub vcf_solving_threats: bool,
    pub vcf_solving_vct: bool,
    pub vcf_cancel_requested: bool,
    pub vcf_started_at: Option<web_time::Instant>,
    pub vcf_elapsed_secs: Option<f64>,
    pub forbidden_points_board: [u8; BOARD_CELLS],
    pub forbidden_points: Vec<usize>,
    pub forbidden_points_valid: bool,
    #[cfg(not(target_arch = "wasm32"))]
    pub vcf_rx: Option<Receiver<VcfJobOutcome>>,
    #[cfg(not(target_arch = "wasm32"))]
    pub vcf_cancel: Option<Arc<AtomicBool>>,
    /// Native progress is coalesced here instead of queued in `vcf_rx`.
    /// The UI only needs the newest ghost line; keeping every intermediate
    /// line can dominate VCT time on a deep DF-PNS search.
    #[cfg(not(target_arch = "wasm32"))]
    pub vcf_progress: Arc<Mutex<Option<Vec<VcfMove>>>>,
    #[cfg(target_arch = "wasm32")]
    pub vcf_worker: Option<web_sys::Worker>,
    #[cfg(target_arch = "wasm32")]
    pub vcf_worker_callback: Option<Closure<dyn FnMut(JsValue)>>,
    #[cfg(target_arch = "wasm32")]
    pub vcf_worker_result: Arc<Mutex<Option<String>>>,
    #[cfg(target_arch = "wasm32")]
    pub wasm_dropped_file: Arc<Mutex<Option<(String, Vec<u8>)>>>,
    #[cfg(target_os = "android")]
    pub android_pending_save: Option<(Vec<u8>, bool, String)>,
}

impl RenjuApp {
    /// Initialize the app with settings.
    #[allow(dead_code)] // Reached from platform entry points excluded from the cdylib target.
    #[allow(dead_code)] // Reached from platform entry points excluded from the cdylib target.
    fn new_with_settings(settings: setting::Settings) -> Self {
        let root_node = RenLibNode::new(-1, -1, NO_NODE, NO_NODE, NO_NODE, 0, 0);
        let mut hash_table = FxHashMap::default();
        hash_table.insert(0, 0);

        Self {
            board: Board::new(),
            lib_nodes: Some(vec![root_node]),
            hash_table: Some(hash_table),
            node_texts: Some(Vec::new()),
            text_pool: Some(TextPool::new()),
            current_node_idx: Some(0),
            is_db_mode: false,
            notation_text: String::new(),
            sgf_text: "(;GM[1]SZ[15])".to_string(),
            portal_v1_text: "https://v1.renjuportal.com/board/?mv=".to_string(),
            portal_v2_text: "https://renjuportal.com/board?mvs=".to_string(),
            settings,
            node_to_delete: None,
            editing_coord: None,
            editing_text: String::new(),
            ad_hoc_labels: FxHashMap::default(),
            current_file: None,
            current_filepath: None,
            load_time_sec: None,
            subtree_cache: std::cell::RefCell::new(FxHashMap::default()),
            is_text_mode: false,
            export_state: ExportState::Idle,
            gif_settings: GifSettings::default(),
            show_gif_setup: false,
            gif_frames: Vec::new(),
            show_about: false,
            show_new_game_confirm: false,
            #[cfg(target_os = "windows")]
            show_vct_log: false,
            copy_notification: None,
            vcf_solution: Vec::new(),
            vcf_replay_len: 0,
            vcf_threats: Vec::new(),
            vcf_status: String::new(),
            vcf_solving: false,
            vcf_solving_threats: false,
            vcf_solving_vct: false,
            vcf_cancel_requested: false,
            vcf_started_at: None,
            vcf_elapsed_secs: None,
            forbidden_points_board: [0; BOARD_CELLS],
            forbidden_points: Vec::new(),
            forbidden_points_valid: false,
            #[cfg(not(target_arch = "wasm32"))]
            vcf_rx: None,
            #[cfg(not(target_arch = "wasm32"))]
            vcf_cancel: None,
            #[cfg(not(target_arch = "wasm32"))]
            vcf_progress: Arc::new(Mutex::new(None)),
            #[cfg(target_arch = "wasm32")]
            vcf_worker: None,
            #[cfg(target_arch = "wasm32")]
            vcf_worker_callback: None,
            #[cfg(target_arch = "wasm32")]
            vcf_worker_result: Arc::new(Mutex::new(None)),
            #[cfg(target_arch = "wasm32")]
            wasm_dropped_file: Arc::new(Mutex::new(None)),
            #[cfg(target_os = "android")]
            android_pending_save: None,
        }
    }

    /// 逶､髱｢縺ｨ繝・Μ繝ｼ繧貞ｮ悟・縺ｫ蛻晄悄蛹悶＠縲∵眠隕丈ｽ懈・迥ｶ諷九↓縺吶ｋ
    pub fn new_game(&mut self) {
        self.board = Board::new();
        self.settings.free_placement = false;
        self.settings.choose_start_player = false;
        self.lib_nodes = Some(vec![RenLibNode::new(
            -1, -1, NO_NODE, NO_NODE, NO_NODE, 0, 0,
        )]);

        let mut hash_table = rustc_hash::FxHashMap::default();
        hash_table.insert(0, 0);
        self.hash_table = Some(hash_table);

        self.node_texts = Some(Vec::new());
        self.text_pool = Some(TextPool::new());
        self.current_node_idx = Some(0);
        self.is_db_mode = false;

        self.current_file = None;
        self.current_filepath = None;
        self.load_time_sec = None;

        // UI繝ｻ謫堺ｽ懃憾諷九・繧ｯ繝ｪ繧｢
        self.node_to_delete = None;
        self.editing_coord = None;
        self.editing_text.clear();
        self.ad_hoc_labels.clear();
        self.subtree_cache.borrow_mut().clear();
        self.export_state = ExportState::Idle;
        self.show_new_game_confirm = false;
        self.clear_vcf_solution();
    }

    /// 隱ｭ縺ｿ霎ｼ縺ｾ繧後◆繝舌う繝医ョ繝ｼ繧ｿ縺九ｉ繝代・繧ｹ繧定｡後＞縲√い繝励Μ繧ｱ繝ｼ繧ｷ繝ｧ繝ｳ迥ｶ諷九ｒ譖ｴ譁ｰ縺吶ｋ
    #[cfg(target_arch = "wasm32")]
    fn process_loaded_data(&mut self, data: &[u8], file_name: String, is_db: bool) {
        let start_time = web_time::Instant::now();
        let encoding = self.settings.text_encoding.to_encoding_rs();

        let mut parser = RenLibParser::new(self.settings.max_nodes);
        parser.set_encoding(encoding);
        let mut parse_board = HashBoard::new();

        let result = if is_db {
            parser.parse_db(data)
        } else {
            parser.parse_all(data, &mut parse_board)
        };

        if result.is_ok() {
            self.current_file = Some(file_name);
            self.lib_nodes = Some(parser.nodes);
            self.hash_table = Some(parser.hash_table);
            self.node_texts = Some(parser.node_texts);
            self.text_pool = Some(parser.text_pool);
            self.subtree_cache.borrow_mut().clear();
            self.current_node_idx = Some(0);
            self.board = Board::new();
            self.settings.free_placement = false;
            self.settings.choose_start_player = false;
            self.is_db_mode = is_db;
            self.load_time_sec = Some(start_time.elapsed().as_secs_f64());

            if !is_db {
                if let Some(nodes) = &self.lib_nodes {
                    let root = &nodes[0];
                    if root.x() >= 0 && root.y() >= 0 {
                        self.board.place_stone(root.x() as usize, root.y() as usize);
                    }
                }
            }
            self.clear_vcf_solution();
        }
    }

    /// 繝輔ぃ繧､繝ｫ繝代せ縺九ｉ諡｡蠑ｵ蟄舌ｒ閾ｪ蜍募愛螳壹＠縺ｦ隱ｭ縺ｿ霎ｼ繧
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_file(&mut self, path: &std::path::Path) {
        let is_db = matches!(path.extension().and_then(|e| e.to_str()), Some("db"));
        self.process_loaded_file(path, is_db);
    }

    #[cfg(target_os = "android")]
    fn process_loaded_reader<R: std::io::Read>(
        &mut self,
        mut reader: R,
        file_name: String,
        is_db: bool,
        current_path: Option<std::path::PathBuf>,
    ) {
        let start_time = web_time::Instant::now();
        let encoding = self.settings.text_encoding.to_encoding_rs();

        let mut parser = RenLibParser::new(self.settings.max_nodes);
        parser.set_encoding(encoding);
        let mut parse_board = HashBoard::new();

        let result = if is_db {
            parser.parse_db_from_reader(reader)
        } else {
            let mut data = Vec::new();
            match reader.read_to_end(&mut data) {
                Ok(_) => parser.parse_all(&data, &mut parse_board),
                Err(err) => Err(err),
            }
        };

        if result.is_ok() {
            self.current_file = Some(file_name);
            self.current_filepath = current_path;
            self.lib_nodes = Some(parser.nodes);
            self.hash_table = Some(parser.hash_table);
            self.node_texts = Some(parser.node_texts);
            self.text_pool = Some(parser.text_pool);
            self.subtree_cache.borrow_mut().clear();
            self.current_node_idx = Some(0);
            self.board = Board::new();
            self.settings.free_placement = false;
            self.settings.choose_start_player = false;
            self.is_db_mode = is_db;
            self.load_time_sec = Some(start_time.elapsed().as_secs_f64());

            if !is_db {
                if let Some(nodes) = &self.lib_nodes {
                    let root = &nodes[0];
                    if root.x() >= 0 && root.y() >= 0 {
                        self.board.place_stone(root.x() as usize, root.y() as usize);
                    }
                }
            }
            self.clear_vcf_solution();
        } else if let Err(err) = result {
            self.vcf_status = format!("File load failed: {err}");
        }
    }

    /// 繝輔ぃ繧､繝ｫ繝代せ縺九ｉ繝・・繧ｿ繧定ｪｭ縺ｿ霎ｼ縺ｿ縲√ヱ繝ｼ繧ｹ繧定｡後≧
    #[cfg(not(target_arch = "wasm32"))]
    pub fn process_loaded_file(&mut self, path: &std::path::Path, is_db: bool) {
        let start_time = web_time::Instant::now();
        let encoding = self.settings.text_encoding.to_encoding_rs();

        let mut parser = RenLibParser::new(self.settings.max_nodes);
        parser.set_encoding(encoding);
        let mut parse_board = HashBoard::new();

        let file_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let result = if let Ok(file) = std::fs::File::open(path) {
            if is_db {
                parser.parse_db_from_reader(file)
            } else {
                // .lib 縺ｯ繝輔ぃ繧､繝ｫ繧ｵ繧､繧ｺ縺悟ｰ上＆縺・parse_all 縺後せ繝ｩ繧､繧ｹ蜑肴署縺ｮ縺溘ａ隱ｭ縺ｿ霎ｼ繧
                if let Ok(data) = std::fs::read(path) {
                    parser.parse_all(&data, &mut parse_board)
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "File read error",
                    ))
                }
            }
        } else {
            return;
        };

        if result.is_ok() {
            self.current_file = Some(file_name);
            self.current_filepath = Some(path.to_path_buf());
            self.lib_nodes = Some(parser.nodes);
            self.hash_table = Some(parser.hash_table);
            self.node_texts = Some(parser.node_texts);
            self.text_pool = Some(parser.text_pool);
            self.subtree_cache.borrow_mut().clear();
            self.current_node_idx = Some(0);
            self.board = Board::new();
            self.settings.free_placement = false;
            self.settings.choose_start_player = false;
            self.is_db_mode = is_db;
            self.load_time_sec = Some(start_time.elapsed().as_secs_f64());

            if !is_db {
                if let Some(nodes) = &self.lib_nodes {
                    let root = &nodes[0];
                    if root.x() >= 0 && root.y() >= 0 {
                        self.board.place_stone(root.x() as usize, root.y() as usize);
                    }
                }
            }
            self.clear_vcf_solution();
        }
    }

    /// Convert the current board to indices.
    #[cfg(target_os = "android")]
    pub fn request_android_open_file(&mut self) {
        match android_file::request_open() {
            Ok(()) => self.vcf_status = "Opening file picker...".to_string(),
            Err(err) => self.vcf_status = format!("Android file picker failed: {err}"),
        }
    }

    #[cfg(target_os = "android")]
    fn request_android_save(&mut self, data: Vec<u8>, branch_only: bool, file_name: String) {
        self.android_pending_save = Some((data, branch_only, file_name.clone()));
        match android_file::request_save(&file_name) {
            Ok(()) => self.vcf_status = "Opening save picker...".to_string(),
            Err(err) => {
                self.android_pending_save = None;
                self.vcf_status = format!("Android save picker failed: {err}");
            }
        }
    }

    #[cfg(target_os = "android")]
    fn process_android_file_events(&mut self, ctx: &egui::Context) {
        match android_file::take_error() {
            Ok(Some(err)) => self.vcf_status = format!("Android file operation failed: {err}"),
            Ok(None) => {}
            Err(err) => self.vcf_status = format!("Android file bridge failed: {err}"),
        }

        match android_file::take_opened_file() {
            Ok(Some(opened)) => {
                let is_db = opened.file_name.to_ascii_lowercase().ends_with(".db");
                self.process_loaded_reader(opened.file, opened.file_name, is_db, None);
            }
            Ok(None) => {}
            Err(err) => self.vcf_status = format!("Android open result failed: {err}"),
        }

        match android_file::take_save_file() {
            Ok(Some(mut file)) => {
                if let Some((data, branch_only, file_name)) = self.android_pending_save.take() {
                    use std::io::Write;
                    match file.write_all(&data).and_then(|_| file.flush()) {
                        Ok(()) => {
                            if !branch_only {
                                self.current_file = Some(file_name);
                                self.current_filepath = None;
                            }
                            self.vcf_status = "File saved".to_string();
                        }
                        Err(err) => self.vcf_status = format!("File save failed: {err}"),
                    }
                }
            }
            Ok(None) => {}
            Err(err) => self.vcf_status = format!("Android save result failed: {err}"),
        }

        match android_file::take_font_selection() {
            Ok(Some((path, name))) => {
                self.settings.font_path = path.clone();
                if !self
                    .settings
                    .available_fonts
                    .iter()
                    .any(|(existing, _)| existing == &path)
                {
                    self.settings.available_fonts.push((path.clone(), name));
                }
                setup_custom_fonts(ctx, &path);
                self.settings.save();
                ctx.request_repaint();
            }
            Ok(None) => {}
            Err(err) => self.vcf_status = format!("Android font result failed: {err}"),
        }
    }

    #[cfg(target_os = "ios")]
    pub fn request_ios_open_file(&mut self) {
        match ios_file::request_open() {
            Ok(()) => self.vcf_status = "Opening iOS document picker...".to_string(),
            Err(err) => self.vcf_status = err,
        }
    }

    #[cfg(target_os = "ios")]
    fn request_ios_save(&mut self, _data: Vec<u8>, _branch_only: bool, file_name: String) {
        match ios_file::request_save(&file_name) {
            Ok(()) => self.vcf_status = "Opening iOS save picker...".to_string(),
            Err(err) => self.vcf_status = err,
        }
    }

    fn get_current_node(&self) -> Option<usize> {
        if self.board.custom_position {
            return None;
        }
        if self.is_db_mode {
            let mut pb = HashBoard::new();
            for &m in &self.board.history {
                pb.make_move((m % 15) as i8, (m / 15) as i8);
            }
            let h = pb.get_canonical_hash();

            if let Some(ht) = &self.hash_table {
                if let Some(&head) = ht.get(&h) {
                    return Some(head as usize);
                }
            }
            None
        } else {
            self.current_node_idx
        }
    }

    /// Reset board to root.
    fn reset_to_root(&mut self) {
        if self.board.custom_position || self.is_db_mode {
            self.board.undo_all();
        } else if let Some(nodes) = &self.lib_nodes {
            while let Some(idx) = self.current_node_idx {
                if idx == 0 {
                    break;
                }
                let node = &nodes[idx];
                if node.x() >= 0 && node.y() >= 0 {
                    self.board.undo();
                }

                if node.parent != NO_NODE {
                    self.current_node_idx = Some(node.parent as usize);
                } else {
                    self.current_node_idx = None;
                }
            }
            if self.current_node_idx.is_none() {
                self.current_node_idx = Some(0);
                self.board.undo_all();
            }
        } else {
            self.board.undo_all();
        }
        self.clear_vcf_solution();
    }

    /// 謖・ｮ壹＆繧後◆蠎ｧ讓吶↓遏ｳ繧呈遠縺｡縲∝ｿ・ｦ√↓蠢懊§縺ｦ譁ｰ縺励＞繝弱・繝峨ｒ繝・Μ繝ｼ縺ｫ霑ｽ蜉縺吶ｋ
    fn play_move(&mut self, ux: usize, uy: usize) {
        if self.settings.free_placement || self.settings.choose_start_player {
            return;
        }
        let before_len = self.board.history.len();
        if self.board.custom_position || self.is_db_mode {
            self.board.place_stone(ux, uy);
        } else if let Some(nodes) = &mut self.lib_nodes {
            if let Some(idx) = self.current_node_idx {
                let mut child_idx = nodes[idx].child;
                let mut matched = false;

                while child_idx != NO_NODE {
                    let c_node = &nodes[child_idx as usize];
                    if c_node.x() == ux as i8 && c_node.y() == uy as i8 {
                        self.current_node_idx = Some(child_idx as usize);
                        self.board.place_stone(ux, uy);
                        matched = true;
                        break;
                    }
                    child_idx = c_node.sibling;
                }

                if !matched {
                    if self.board.place_stone(ux, uy) {
                        let new_hash = {
                            let mut pb = HashBoard::new();
                            for &m in &self.board.history {
                                pb.make_move((m % 15) as i8, (m / 15) as i8);
                            }
                            pb.get_canonical_hash()
                        };

                        let new_idx = nodes.len();
                        nodes.push(RenLibNode::new(
                            ux as i8,
                            uy as i8,
                            idx as u32,
                            NO_NODE,
                            nodes[idx].child,
                            new_hash,
                            nodes[idx].depth() + 1,
                        ));

                        nodes[idx].child = new_idx as u32;
                        self.current_node_idx = Some(new_idx);

                        if let Some(ht) = &mut self.hash_table {
                            ht.insert(new_hash, new_idx as u32);
                        }
                        self.subtree_cache.borrow_mut().clear();
                    }
                }
            }
        }
        if self.board.history.len() != before_len {
            self.clear_vcf_solution();
        }
    }

    /// 迴ｾ蝨ｨ驕ｸ謚槭＆繧後※縺・ｋ蛻・ｲ舌→縺昴・蟄仙ｭｫ繧偵☆縺ｹ縺ｦ蜑企勁縺吶ｋ
    fn delete_current_branch(&mut self) {
        if let Some(idx) = self.node_to_delete {
            let is_current = self.get_current_node() == Some(idx);
            let before_len = self.board.history.len();

            if let Some(nodes) = &mut self.lib_nodes {
                if idx > 0 && idx < nodes.len() {
                    let mut subtree_indices = Vec::new();

                    // 繧ｹ繧ｿ繝・け繧剃ｽｿ逕ｨ縺励※蟄仙ｭｫ繝弱・繝峨ｒ蜀榊ｸｰ逧・↓蜿朱寔縺吶ｋ
                    fn collect_subtree(nodes: &[RenLibNode], start: usize, out: &mut Vec<usize>) {
                        out.push(start);
                        let mut curr = nodes[start].child;
                        while curr != NO_NODE {
                            collect_subtree(nodes, curr as usize, out);
                            curr = nodes[curr as usize].sibling;
                        }
                    }
                    collect_subtree(nodes, idx, &mut subtree_indices);

                    if let Some(ht) = &mut self.hash_table {
                        for &s_idx in &subtree_indices {
                            let h = nodes[s_idx].hash;
                            if let Some(&head) = ht.get(&h) {
                                if head == s_idx as u32 {
                                    ht.remove(&h);
                                }
                            }
                        }
                    }

                    let p_idx_u32 = nodes[idx].parent;
                    if p_idx_u32 != NO_NODE {
                        let p_idx = p_idx_u32 as usize;
                        if nodes[p_idx].child == idx as u32 {
                            nodes[p_idx].child = nodes[idx].sibling;
                        } else {
                            let mut curr = nodes[p_idx].child as usize;
                            while nodes[curr].sibling != NO_NODE {
                                if nodes[curr].sibling == idx as u32 {
                                    nodes[curr].sibling = nodes[idx].sibling;
                                    break;
                                }
                                curr = nodes[curr].sibling as usize;
                            }
                        }
                    }

                    for &s_idx in &subtree_indices {
                        nodes[s_idx].set_x(-2);
                        nodes[s_idx].set_y(-2);
                    }

                    if is_current {
                        self.board.undo();
                        if !self.is_db_mode {
                            let p_idx = nodes[idx].parent;
                            if p_idx != NO_NODE {
                                self.current_node_idx = Some(p_idx as usize);
                            } else {
                                self.current_node_idx = None;
                            }
                        }
                    }
                }
            }
            self.subtree_cache.borrow_mut().clear();
            self.node_to_delete = None;
            if self.board.history.len() != before_len {
                self.clear_vcf_solution();
            }
        }
    }

    /// 螻･豁ｴ縺ｾ縺溘・蜊倅ｸ縺ｮ蛻・ｲ舌↓蝓ｺ縺･縺・※荳謇矩ｲ繧√ｋ
    fn step_forward(&mut self) -> bool {
        if self.settings.free_placement || self.settings.choose_start_player {
            return false;
        }
        if let Some(&next_pos) = self.board.future_history.last() {
            self.board.redo();
            if !self.board.custom_position && !self.is_db_mode {
                if let (Some(nodes), Some(idx)) = (&self.lib_nodes, self.current_node_idx) {
                    let mut c = nodes[idx].child;
                    while c != NO_NODE {
                        let n = &nodes[c as usize];
                        if n.x() == (next_pos % 15) as i8 && n.y() == (next_pos / 15) as i8 {
                            self.current_node_idx = Some(c as usize);
                            break;
                        }
                        c = n.sibling;
                    }
                }
            }
            self.clear_vcf_solution();
            return true;
        }

        if self.board.custom_position {
            return false;
        }

        // 2. 螻･豁ｴ縺檎┌縺上√°縺､逋ｻ骭ｲ縺輔ｌ縺ｦ縺・ｋ蛻・ｲ舌′1縺､縺縺代↑繧峨◎縺薙∈閾ｪ蜍慕噪縺ｫ騾ｲ繧
        if self.is_db_mode {
            // DB繝｢繝ｼ繝・ 逶､髱｢縺ｮ蜈ｨ遨ｺ縺阪・繧ｹ縺ｫ莉ｮ鄂ｮ縺阪＠縲√ワ繝・す繝･霎樊嶌繧呈､懃ｴ｢縺励※蛻・ｲ舌ｒ謨ｰ縺医ｋ
            let mut valid_moves = Vec::new();
            if let Some(ht) = &self.hash_table {
                let mut pb = HashBoard::new();
                for &m in &self.board.history {
                    pb.make_move((m % 15) as i8, (m / 15) as i8);
                }

                for y in 0..15 {
                    for x in 0..15 {
                        if self.board.grid[y * 15 + x].is_none() {
                            pb.make_move(x as i8, y as i8);
                            let h = pb.get_canonical_hash();
                            pb.undo_move(x as i8, y as i8);

                            if ht.contains_key(&h) {
                                valid_moves.push((x, y));
                            }
                        }
                    }
                }
            }

            // 蛻・ｲ舌′縺｡繧・≧縺ｩ1縺､縺ｮ蝣ｴ蜷医・縺ｿ閾ｪ蜍輔〒謇九ｒ騾ｲ繧√ｋ
            if valid_moves.len() == 1 {
                self.board.place_stone(valid_moves[0].0, valid_moves[0].1);
                self.clear_vcf_solution();
                return true;
            }
        } else {
            if let (Some(nodes), Some(idx)) = (&self.lib_nodes, self.current_node_idx) {
                let first_child = nodes[idx].child;
                if first_child != NO_NODE && nodes[first_child as usize].sibling == NO_NODE {
                    let n = &nodes[first_child as usize];
                    if n.x() >= 0 && n.y() >= 0 {
                        self.board.place_stone(n.x() as usize, n.y() as usize);
                        self.current_node_idx = Some(first_child as usize);
                        self.clear_vcf_solution();
                        return true;
                    }
                }
            }
        }
        false
    }

    // 迴ｾ蝨ｨ縺ｮ繝弱・繝峨↓閾ｳ繧九∪縺ｧ縺ｮ逶ｴ蛻励↑螻･豁ｴ縺ｨ縲∫樟蝨ｨ縺ｮ繝弱・繝我ｻ･荳九・蜈ｨ蛻・ｲ舌ｒ謚ｽ蜃ｺ縺吶ｋ
    /// Get branch nodes.
    fn get_branch_nodes(&self) -> Option<Vec<RenLibNode>> {
        let current_idx = self.get_current_node()?;
        let original = self.lib_nodes.as_ref()?;
        let mut nodes = original.clone();

        // 蜈ｨ縺ｦ縺ｮ繝弱・繝峨ｒ荳譌ｦ縲悟炎髯､迥ｶ諷・(-2, -2)縲阪↓縺吶ｋ
        for node in nodes.iter_mut() {
            node.set_x(-2);
            node.set_y(-2);
        }

        let mut stack = vec![current_idx];
        while let Some(idx) = stack.pop() {
            let orig_node = &original[idx];
            nodes[idx].set_x(orig_node.x());
            nodes[idx].set_y(orig_node.y());

            let mut child = orig_node.child;
            while child != NO_NODE {
                stack.push(child as usize);
                child = original[child as usize].sibling;
            }
        }

        let mut curr = current_idx;
        while curr != 0 && curr != NO_NODE as usize {
            let p_u32 = original[curr].parent;
            if p_u32 != NO_NODE {
                let p = p_u32 as usize;
                nodes[p].set_x(original[p].x());
                nodes[p].set_y(original[p].y());
                curr = p;
            } else {
                break;
            }
        }

        if !original.is_empty() {
            nodes[0].set_x(original[0].x());
            nodes[0].set_y(original[0].y());
        }

        Some(nodes)
    }

    /// Show save file dialog.
    fn save_file_dialog(&mut self, branch_only: bool) {
        let nodes_to_save = if branch_only {
            self.get_branch_nodes()
        } else {
            self.lib_nodes.clone()
        };

        if let (Some(nodes), Some(pool), Some(node_texts)) =
            (nodes_to_save, &self.text_pool, &self.node_texts)
        {
            #[cfg(all(
                not(target_arch = "wasm32"),
                not(target_os = "android"),
                not(target_os = "ios")
            ))]
            {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("RenLib files", &["lib"])
                    .add_filter("Yixin database", &["db"])
                    .save_file()
                {
                    let is_db = path.extension().and_then(|s| s.to_str()) == Some("db");
                    let encoding = self.settings.text_encoding.to_encoding_rs();
                    let mut writer = RenLibWriter::new(encoding);
                    let data = if is_db {
                        writer.save_db(&nodes, node_texts, pool)
                    } else {
                        writer.save(&nodes, node_texts, pool)
                    };
                    if std::fs::write(&path, data).is_ok() && !branch_only {
                        // 蜈ｨ菴謎ｿ晏ｭ倥・蝣ｴ蜷医・迴ｾ蝨ｨ縺ｮ繝輔ぃ繧､繝ｫ繝代せ繧呈峩譁ｰ縺吶ｋ
                        self.current_filepath = Some(path.clone());
                        self.current_file = Some(
                            path.file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("Unknown")
                                .to_string(),
                        );
                    }
                }
            }
            #[cfg(target_os = "android")]
            {
                let encoding = self.settings.text_encoding.to_encoding_rs();
                let mut writer = RenLibWriter::new(encoding);
                let save_as_db = self.is_db_mode && !branch_only;
                let data = if save_as_db {
                    writer.save_db(&nodes, node_texts, pool)
                } else {
                    writer.save(&nodes, node_texts, pool)
                };
                let fallback_name = if branch_only {
                    "saved_branch.lib"
                } else if save_as_db {
                    "saved_game.db"
                } else {
                    "saved_game.lib"
                };
                let file_name = self
                    .current_file
                    .clone()
                    .filter(|_| !branch_only)
                    .unwrap_or_else(|| fallback_name.to_string());
                self.request_android_save(data, branch_only, file_name);
            }
            #[cfg(target_os = "ios")]
            {
                let encoding = self.settings.text_encoding.to_encoding_rs();
                let mut writer = RenLibWriter::new(encoding);
                let save_as_db = self.is_db_mode && !branch_only;
                let data = if save_as_db {
                    writer.save_db(&nodes, node_texts, pool)
                } else {
                    writer.save(&nodes, node_texts, pool)
                };
                let fallback_name = if branch_only {
                    "saved_branch.lib"
                } else if save_as_db {
                    "saved_game.db"
                } else {
                    "saved_game.lib"
                };
                let file_name = self
                    .current_file
                    .clone()
                    .filter(|_| !branch_only)
                    .unwrap_or_else(|| fallback_name.to_string());
                self.request_ios_save(data, branch_only, file_name);
            }
            #[cfg(target_arch = "wasm32")]
            {
                let encoding = self.settings.text_encoding.to_encoding_rs();
                let mut writer = RenLibWriter::new(encoding);
                let data = writer.save(&nodes, node_texts, pool);
                wasm_bindgen_futures::spawn_local(async move {
                    if let Some(handle) = rfd::AsyncFileDialog::new()
                        .add_filter("RenLib files", &["lib"])
                        .set_file_name("saved_game.lib")
                        .save_file()
                        .await
                    {
                        let _ = handle.write(&data).await;
                    }
                });
            }
        }
    }

    /// Overwrite the current file.
    fn overwrite_save(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let (Some(path), Some(nodes), Some(pool), Some(node_texts)) = (
                &self.current_filepath,
                &self.lib_nodes,
                &self.text_pool,
                &self.node_texts,
            ) {
                let is_db = path.extension().and_then(|s| s.to_str()) == Some("db");
                let encoding = self.settings.text_encoding.to_encoding_rs();
                let mut writer = RenLibWriter::new(encoding);
                let data = if is_db {
                    writer.save_db(nodes, node_texts, pool)
                } else {
                    writer.save(nodes, node_texts, pool)
                };
                let _ = std::fs::write(path, data);
            } else {
                self.save_file_dialog(false);
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.save_file_dialog(false);
        }
    }

    /// 繧ｳ繝斐・騾夂衍繧定｡ｨ遉ｺ縺吶ｋ
    pub fn trigger_copy_notification(&mut self) {
        let tr = self.settings.tr();
        self.copy_notification =
            Some((tr.copied_to_clipboard.to_string(), web_time::Instant::now()));
    }

    pub fn copy_text_to_clipboard(&mut self, _ui: &mut egui::Ui, text: &str) {
        #[cfg(target_os = "android")]
        {
            if let Err(err) = android_file::set_clipboard_text(text) {
                self.vcf_status = format!("Clipboard copy failed: {err}");
                return;
            }
        }

        #[cfg(not(target_os = "android"))]
        {
            _ui.output_mut(|o| o.copied_text = text.to_string());
        }

        self.trigger_copy_notification();
    }

    pub fn paste_text_from_clipboard(&mut self, _ctx: &egui::Context) -> Option<String> {
        #[cfg(target_os = "android")]
        {
            match android_file::get_clipboard_text() {
                Ok(text) => text,
                Err(err) => {
                    self.vcf_status = format!("Clipboard paste failed: {err}");
                    None
                }
            }
        }

        #[cfg(not(target_os = "android"))]
        {
            _ctx.input(|i| {
                i.events.iter().find_map(|e| {
                    if let egui::Event::Paste(s) = e {
                        Some(s.clone())
                    } else {
                        None
                    }
                })
            })
        }
    }

    #[cfg(target_os = "android")]
    pub fn android_safe_inset_top_points(&mut self, ctx: &egui::Context) -> f32 {
        match android_file::safe_inset_top() {
            Ok(px) => (px.max(0) as f32 / ctx.pixels_per_point()).min(80.0),
            Err(_) => 0.0,
        }
    }

    pub fn clear_vcf_solution(&mut self) {
        self.forbidden_points_valid = false;
        #[cfg(not(target_arch = "wasm32"))]
        if self.vcf_solving {
            if let Some(cancel) = &self.vcf_cancel {
                cancel.store(true, Ordering::Relaxed);
            }
            self.vcf_rx = None;
            self.vcf_cancel = None;
            if let Ok(mut progress) = self.vcf_progress.lock() {
                *progress = None;
            }
            self.vcf_solving = false;
            self.vcf_solving_threats = false;
            self.vcf_solving_vct = false;
            self.vcf_cancel_requested = false;
            self.vcf_started_at = None;
            self.vcf_elapsed_secs = None;
        }

        #[cfg(target_arch = "wasm32")]
        if self.vcf_solving {
            if let Some(worker) = self.vcf_worker.take() {
                worker.terminate();
            }
            self.vcf_worker_callback = None;
            if let Ok(mut result) = self.vcf_worker_result.lock() {
                *result = None;
            }
            self.vcf_solving = false;
            self.vcf_solving_threats = false;
            self.vcf_solving_vct = false;
            self.vcf_cancel_requested = false;
            self.vcf_started_at = None;
            self.vcf_elapsed_secs = None;
        }

        self.vcf_solution.clear();
        self.vcf_replay_len = 0;
        self.vcf_threats.clear();
        self.vcf_started_at = None;
        self.vcf_elapsed_secs = None;
        self.vcf_solving_threats = false;
        self.vcf_solving_vct = false;
        if !self.vcf_solving {
            self.vcf_status.clear();
        }
    }

    pub fn current_vcf_board(&self) -> [u8; BOARD_CELLS] {
        let mut board = [0; BOARD_CELLS];
        for (idx, player) in self.board.grid.iter().enumerate() {
            board[idx] = match player {
                Some(crate::board::Player::Black) => BLACK,
                Some(crate::board::Player::White) => crate::vcf::solver::WHITE,
                None => 0,
            };
        }
        board
    }

    fn search_side_to_move(&self, board: &[u8; BOARD_CELLS]) -> Result<u8, String> {
        if self.board.custom_position {
            Ok(match self.board.next_player {
                crate::board::Player::Black => BLACK,
                crate::board::Player::White => crate::vcf::solver::WHITE,
            })
        } else {
            side_to_move(board)
        }
    }

    pub fn cached_forbidden_points(&mut self) -> &[usize] {
        let board = self.current_vcf_board();
        if !self.forbidden_points_valid || self.forbidden_points_board != board {
            self.forbidden_points = if self.board.custom_position {
                if self.settings.free_placement
                    || self.board.next_player == crate::board::Player::Black
                {
                    Solver::black_forbidden_points_unchecked(board)
                } else {
                    Vec::new()
                }
            } else {
                Solver::black_forbidden_points(board)
            };
            self.forbidden_points_board = board;
            self.forbidden_points_valid = true;
        }
        &self.forbidden_points
    }

    pub fn start_vcf_search(&mut self) {
        if self.vcf_solving {
            return;
        }
        let mode = search_mode_from_setting_for_vcf(self.settings.vcf_mode);
        let board = self.current_vcf_board();
        let turn = match self.search_side_to_move(&board) {
            Ok(turn) => turn,
            Err(error) => {
                self.vcf_status = error;
                self.vcf_solution.clear();
                return;
            }
        };
        let depth = self.settings.vcf_depth.max(1);
        let timeout_secs = self.settings.vcf_timeout_secs.max(1);
        self.vcf_solution.clear();
        self.vcf_replay_len = 0;
        self.vcf_threats.clear();
        self.vcf_solving = true;
        self.vcf_solving_threats = false;
        self.vcf_solving_vct = false;
        self.vcf_cancel_requested = false;
        self.vcf_started_at = Some(web_time::Instant::now());
        self.vcf_elapsed_secs = None;
        self.vcf_status = format!(
            "{} {} / depth {} / {}s",
            if turn == BLACK { "Black" } else { "White" },
            if mode == SearchMode::Long {
                "VCF long"
            } else {
                "VCF shortest"
            },
            depth,
            timeout_secs
        );

        #[cfg(not(target_arch = "wasm32"))]
        {
            let timeout = std::time::Duration::from_secs(timeout_secs);
            let (tx, rx) = std::sync::mpsc::channel();
            let cancel = Arc::new(AtomicBool::new(false));
            let worker_cancel = cancel.clone();
            std::thread::spawn(move || {
                let outcome =
                    Solver::solve_with_cancel(board, turn, depth, timeout, worker_cancel, mode);
                let _ = tx.send(VcfJobOutcome::Solve(outcome));
            });
            self.vcf_rx = Some(rx);
            self.vcf_cancel = Some(cancel);
        }

        #[cfg(target_arch = "wasm32")]
        {
            let request = VcfWorkerRequest {
                job: "solve".to_string(),
                board: board.to_vec(),
                turn,
                depth,
                timeout_secs,
                mode: search_mode_wire(mode).to_string(),
            };

            let request_json = match serde_json::to_string(&request) {
                Ok(json) => json,
                Err(error) => {
                    self.vcf_solving = false;
                    self.vcf_solving_threats = false;
                    self.vcf_solving_vct = false;
                    self.finish_vcf_timer();
                    self.vcf_status = format!("VCF worker request failed: {error}");
                    return;
                }
            };

            if let Ok(mut result) = self.vcf_worker_result.lock() {
                *result = None;
            }

            let result_slot = self.vcf_worker_result.clone();
            let callback = Closure::wrap(Box::new(move |value: JsValue| {
                let message = value.as_string().unwrap_or_else(|| {
                    "{\"kind\":\"Error\",\"message\":\"VCF worker returned a non-string response\"}"
                        .to_string()
                });
                if let Ok(mut result) = result_slot.lock() {
                    *result = Some(message);
                }
            }) as Box<dyn FnMut(JsValue)>);

            match spawn_vcf_worker(&request_json, callback.as_ref().unchecked_ref()) {
                Ok(worker) => {
                    self.vcf_worker = Some(worker);
                    self.vcf_worker_callback = Some(callback);
                }
                Err(error) => {
                    self.vcf_solving = false;
                    self.vcf_solving_threats = false;
                    self.vcf_solving_vct = false;
                    self.finish_vcf_timer();
                    self.vcf_status = format!("VCF worker start failed: {error:?}");
                }
            }
        }
    }

    pub fn start_vct_search(&mut self) {
        if self.vcf_solving {
            return;
        }
        // Quintet owns VCT move generation and its internal search policy;
        // users configure only the shared depth and wall-clock bounds.
        let mode = SearchMode::Fast;
        let board = self.current_vcf_board();
        let turn = match self.search_side_to_move(&board) {
            Ok(turn) => turn,
            Err(error) => {
                self.vcf_status = error;
                self.vcf_solution.clear();
                self.vcf_threats.clear();
                return;
            }
        };
        let depth = self.settings.vct_depth.max(1);
        let timeout_secs = self.settings.vct_timeout_secs.max(1);
        self.vcf_solution.clear();
        self.vcf_replay_len = 0;
        self.vcf_threats.clear();
        self.vcf_solving = true;
        self.vcf_solving_threats = false;
        self.vcf_solving_vct = true;
        self.vcf_cancel_requested = false;
        self.vcf_started_at = Some(web_time::Instant::now());
        self.vcf_elapsed_secs = None;
        self.vcf_status = format!(
            "{} VCT (Quintet) / depth {} / {}s",
            if turn == BLACK { "Black" } else { "White" },
            depth,
            timeout_secs
        );

        #[cfg(not(target_arch = "wasm32"))]
        {
            let timeout = std::time::Duration::from_secs(timeout_secs);
            let (tx, rx) = std::sync::mpsc::channel();
            let cancel = Arc::new(AtomicBool::new(false));
            let worker_cancel = cancel.clone();
            if let Ok(mut progress) = self.vcf_progress.lock() {
                *progress = None;
            }
            let progress_slot = self.vcf_progress.clone();
            std::thread::spawn(move || {
                let outcome = Solver::solve_vct_with_progress(
                    board,
                    turn,
                    depth,
                    timeout,
                    worker_cancel,
                    mode,
                    move |path| {
                        if let Ok(mut latest) = progress_slot.lock() {
                            *latest = Some(path);
                        }
                    },
                );
                let _ = tx.send(VcfJobOutcome::Vct(outcome));
            });
            self.vcf_rx = Some(rx);
            self.vcf_cancel = Some(cancel);
        }

        #[cfg(target_arch = "wasm32")]
        {
            let request = VcfWorkerRequest {
                job: "vct".to_string(),
                board: board.to_vec(),
                turn,
                depth,
                timeout_secs,
                mode: search_mode_wire(mode).to_string(),
            };

            let request_json = match serde_json::to_string(&request) {
                Ok(json) => json,
                Err(error) => {
                    self.vcf_solving = false;
                    self.vcf_solving_threats = false;
                    self.vcf_solving_vct = false;
                    self.finish_vcf_timer();
                    self.vcf_status = format!("VCT worker request failed: {error}");
                    return;
                }
            };

            if let Ok(mut result) = self.vcf_worker_result.lock() {
                *result = None;
            }

            let result_slot = self.vcf_worker_result.clone();
            let callback = Closure::wrap(Box::new(move |value: JsValue| {
                let message = value.as_string().unwrap_or_else(|| {
                    "{\"kind\":\"Error\",\"message\":\"VCF worker returned a non-string response\"}"
                        .to_string()
                });
                if let Ok(mut result) = result_slot.lock() {
                    *result = Some(message);
                }
            }) as Box<dyn FnMut(JsValue)>);

            match spawn_vcf_worker(&request_json, callback.as_ref().unchecked_ref()) {
                Ok(worker) => {
                    self.vcf_worker = Some(worker);
                    self.vcf_worker_callback = Some(callback);
                }
                Err(error) => {
                    self.vcf_solving = false;
                    self.vcf_solving_threats = false;
                    self.vcf_solving_vct = false;
                    self.finish_vcf_timer();
                    self.vcf_status = format!("VCT worker start failed: {error:?}");
                }
            }
        }
    }

    pub fn start_vcf_threat_search(&mut self) {
        if self.vcf_solving {
            return;
        }
        let mode = search_mode_from_setting_for_vcf(self.settings.vcf_mode);
        let board = self.current_vcf_board();
        let turn = match self.search_side_to_move(&board) {
            Ok(turn) => turn,
            Err(error) => {
                self.vcf_status = error;
                self.vcf_solution.clear();
                self.vcf_threats.clear();
                return;
            }
        };
        let depth = self.settings.vcf_depth.max(1);
        let timeout_secs = self.settings.vcf_timeout_secs.max(1);
        self.vcf_solution.clear();
        self.vcf_replay_len = 0;
        self.vcf_threats.clear();
        self.vcf_solving = true;
        self.vcf_solving_threats = true;
        self.vcf_solving_vct = false;
        self.vcf_cancel_requested = false;
        self.vcf_started_at = Some(web_time::Instant::now());
        self.vcf_elapsed_secs = None;
        self.vcf_status = format!(
            "{} VCF threat / depth {} / {}s",
            if turn == BLACK { "Black" } else { "White" },
            depth,
            timeout_secs
        );

        #[cfg(not(target_arch = "wasm32"))]
        {
            let timeout = std::time::Duration::from_secs(timeout_secs);
            let (tx, rx) = std::sync::mpsc::channel();
            let cancel = Arc::new(AtomicBool::new(false));
            let worker_cancel = cancel.clone();
            std::thread::spawn(move || {
                let outcome =
                    find_vcf_threats(board, turn, depth, timeout, mode, Some(worker_cancel));
                let _ = tx.send(VcfJobOutcome::Threats(outcome));
            });
            self.vcf_rx = Some(rx);
            self.vcf_cancel = Some(cancel);
        }

        #[cfg(target_arch = "wasm32")]
        {
            let request = VcfWorkerRequest {
                job: "threats".to_string(),
                board: board.to_vec(),
                turn,
                depth,
                timeout_secs,
                mode: search_mode_wire(mode).to_string(),
            };

            let request_json = match serde_json::to_string(&request) {
                Ok(json) => json,
                Err(error) => {
                    self.vcf_solving = false;
                    self.vcf_solving_threats = false;
                    self.vcf_solving_vct = false;
                    self.finish_vcf_timer();
                    self.vcf_status = format!("VCF threat worker request failed: {error}");
                    return;
                }
            };

            if let Ok(mut result) = self.vcf_worker_result.lock() {
                *result = None;
            }

            let result_slot = self.vcf_worker_result.clone();
            let callback = Closure::wrap(Box::new(move |value: JsValue| {
                let message = value.as_string().unwrap_or_else(|| {
                    "{\"kind\":\"Error\",\"message\":\"VCF worker returned a non-string response\"}"
                        .to_string()
                });
                if let Ok(mut result) = result_slot.lock() {
                    *result = Some(message);
                }
            }) as Box<dyn FnMut(JsValue)>);

            match spawn_vcf_worker(&request_json, callback.as_ref().unchecked_ref()) {
                Ok(worker) => {
                    self.vcf_worker = Some(worker);
                    self.vcf_worker_callback = Some(callback);
                }
                Err(error) => {
                    self.vcf_solving = false;
                    self.vcf_solving_threats = false;
                    self.vcf_solving_vct = false;
                    self.finish_vcf_timer();
                    self.vcf_status = format!("VCF threat worker start failed: {error:?}");
                }
            }
        }
    }

    pub fn current_vcf_elapsed_secs(&self) -> Option<f64> {
        if self.vcf_solving {
            self.vcf_started_at
                .map(|started| started.elapsed().as_secs_f64())
        } else {
            self.vcf_elapsed_secs
        }
    }

    fn finish_vcf_timer(&mut self) -> Option<f64> {
        let elapsed = self
            .vcf_started_at
            .take()
            .map(|started| started.elapsed().as_secs_f64())
            .or(self.vcf_elapsed_secs);
        self.vcf_elapsed_secs = elapsed;
        elapsed
    }

    pub fn cancel_vcf_search(&mut self) {
        if !self.vcf_solving {
            return;
        }
        self.vcf_cancel_requested = true;

        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(cancel) = &self.vcf_cancel {
                cancel.store(true, Ordering::Relaxed);
            }
            self.vcf_rx = None;
            self.vcf_cancel = None;
        }

        #[cfg(target_arch = "wasm32")]
        {
            if let Some(worker) = self.vcf_worker.take() {
                worker.terminate();
            }
            self.vcf_worker_callback = None;
            if let Ok(mut result) = self.vcf_worker_result.lock() {
                *result = None;
            }
        }

        if self.vcf_solving_threats {
            self.finish_vcf_threat_search(ThreatSearchOutcome::Cancelled);
        } else if self.vcf_solving_vct {
            self.finish_vct_search(SolveOutcome::Cancelled);
        } else {
            self.finish_vcf_search(SolveOutcome::Cancelled);
        }
    }

    pub fn vcf_replay_to_start(&mut self) {
        if !self.vcf_solution.is_empty() {
            self.vcf_replay_len = 0;
        }
    }

    pub fn vcf_replay_prev(&mut self) {
        self.vcf_replay_len = self.vcf_replay_len.saturating_sub(1);
    }

    pub fn vcf_replay_next(&mut self) {
        if !self.vcf_solution.is_empty() {
            self.vcf_replay_len = (self.vcf_replay_len + 1).min(self.vcf_solution.len());
        }
    }

    pub fn vcf_replay_to_end(&mut self) {
        self.vcf_replay_len = self.vcf_solution.len();
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn poll_vcf_search(&mut self, ctx: &egui::Context) {
        let progress = self
            .vcf_progress
            .lock()
            .ok()
            .and_then(|mut latest| latest.take());
        if let Some(path) = progress {
            self.show_vcf_progress(path);
            ctx.request_repaint();
        }

        if let Some(rx) = &self.vcf_rx {
            match rx.try_recv() {
                Ok(outcome) => {
                    self.vcf_rx = None;
                    self.vcf_cancel = None;
                    self.finish_vcf_job(outcome);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(80));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.vcf_rx = None;
                    self.vcf_cancel = None;
                    self.vcf_solving = false;
                    self.vcf_solving_threats = false;
                    self.vcf_solving_vct = false;
                    self.vcf_cancel_requested = false;
                    self.finish_vcf_timer();
                    self.vcf_status = "VCF search thread ended unexpectedly".to_string();
                }
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn poll_vcf_search(&mut self, ctx: &egui::Context) {
        let response_json = self
            .vcf_worker_result
            .lock()
            .ok()
            .and_then(|mut result| result.take());

        if let Some(response_json) = response_json {
            match serde_json::from_str::<VcfWorkerResponse>(&response_json) {
                Ok(VcfWorkerResponse::Found { moves }) => {
                    self.vcf_worker = None;
                    self.vcf_worker_callback = None;
                    self.finish_vcf_search(SolveOutcome::Found(moves));
                }
                Ok(VcfWorkerResponse::VctFound { moves }) => {
                    self.vcf_worker = None;
                    self.vcf_worker_callback = None;
                    self.finish_vct_search(SolveOutcome::Found(moves));
                }
                Ok(VcfWorkerResponse::TacticalVctFound { moves }) => {
                    self.vcf_worker = None;
                    self.vcf_worker_callback = None;
                    self.finish_vct_search(SolveOutcome::TacticalFound(moves));
                }
                Ok(VcfWorkerResponse::Threats { moves }) => {
                    self.vcf_worker = None;
                    self.vcf_worker_callback = None;
                    self.finish_vcf_threat_search(ThreatSearchOutcome::Complete(moves));
                }
                Ok(VcfWorkerResponse::Progress { moves }) => {
                    self.show_vcf_progress(moves);
                }
                Ok(VcfWorkerResponse::NotFound) => {
                    self.vcf_worker = None;
                    self.vcf_worker_callback = None;
                    if self.vcf_solving_vct {
                        self.finish_vct_search(SolveOutcome::NotFound);
                    } else {
                        self.finish_vcf_search(SolveOutcome::NotFound);
                    }
                }
                Ok(VcfWorkerResponse::Incomplete) => {
                    self.vcf_worker = None;
                    self.vcf_worker_callback = None;
                    if self.vcf_solving_vct {
                        self.finish_vct_search(SolveOutcome::Incomplete);
                    } else {
                        self.finish_vcf_search(SolveOutcome::Incomplete);
                    }
                }
                Ok(VcfWorkerResponse::Timeout) => {
                    self.vcf_worker = None;
                    self.vcf_worker_callback = None;
                    if self.vcf_solving_threats {
                        self.finish_vcf_threat_search(ThreatSearchOutcome::Partial(Vec::new()));
                    } else if self.vcf_solving_vct {
                        self.finish_vct_search(SolveOutcome::Timeout);
                    } else {
                        self.finish_vcf_search(SolveOutcome::Timeout);
                    }
                }
                Ok(VcfWorkerResponse::Cancelled) => {
                    self.vcf_worker = None;
                    self.vcf_worker_callback = None;
                    if self.vcf_solving_threats {
                        self.finish_vcf_threat_search(ThreatSearchOutcome::Cancelled);
                    } else if self.vcf_solving_vct {
                        self.finish_vct_search(SolveOutcome::Cancelled);
                    } else {
                        self.finish_vcf_search(SolveOutcome::Cancelled);
                    }
                }
                Ok(VcfWorkerResponse::Error { message }) => {
                    self.vcf_worker = None;
                    self.vcf_worker_callback = None;
                    self.vcf_solving = false;
                    self.vcf_solving_threats = false;
                    self.vcf_solving_vct = false;
                    self.finish_vcf_timer();
                    self.vcf_solution.clear();
                    self.vcf_threats.clear();
                    self.vcf_status = format!("VCF worker error: {message}");
                }
                Err(error) => {
                    self.vcf_worker = None;
                    self.vcf_worker_callback = None;
                    self.vcf_solving = false;
                    self.vcf_solving_threats = false;
                    self.vcf_solving_vct = false;
                    self.finish_vcf_timer();
                    self.vcf_solution.clear();
                    self.vcf_threats.clear();
                    self.vcf_status = format!("VCF worker response failed: {error}");
                }
            }
            ctx.request_repaint();
        } else if self.vcf_solving {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn finish_vcf_job(&mut self, outcome: VcfJobOutcome) {
        match outcome {
            VcfJobOutcome::Solve(outcome) => self.finish_vcf_search(outcome),
            VcfJobOutcome::Vct(outcome) => self.finish_vct_search(outcome),
            VcfJobOutcome::Threats(outcome) => self.finish_vcf_threat_search(outcome),
        }
    }

    fn show_vcf_progress(&mut self, path: Vec<VcfMove>) {
        if !self.vcf_solving_vct {
            return;
        }
        if path.is_empty() {
            self.vcf_solution.clear();
            self.vcf_replay_len = 0;
            return;
        }
        self.vcf_solution = path;
        self.vcf_replay_len = self.vcf_solution.len();
    }

    fn finish_vcf_threat_search(&mut self, outcome: ThreatSearchOutcome) {
        self.vcf_solving = false;
        self.vcf_solving_threats = false;
        self.vcf_solving_vct = false;
        self.vcf_cancel_requested = false;
        let elapsed = self.finish_vcf_timer();
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.vcf_cancel = None;
        }

        self.vcf_solution.clear();
        self.vcf_replay_len = 0;

        match outcome {
            ThreatSearchOutcome::Complete(moves) => {
                self.vcf_status =
                    format_vcf_status("VCF threats found", Some(moves.len()), elapsed);
                self.vcf_threats = moves;
            }
            ThreatSearchOutcome::Partial(moves) => {
                self.vcf_status =
                    format_vcf_status("VCF threat search incomplete", Some(moves.len()), elapsed);
                self.vcf_threats = moves;
            }
            ThreatSearchOutcome::Cancelled => {
                self.vcf_status = format_vcf_status("VCF threat search cancelled", None, elapsed);
                self.vcf_threats.clear();
            }
        }
    }

    fn finish_vcf_search(&mut self, outcome: SolveOutcome) {
        self.vcf_solving = false;
        self.vcf_solving_threats = false;
        self.vcf_solving_vct = false;
        self.vcf_cancel_requested = false;
        let elapsed = self.finish_vcf_timer();
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.vcf_cancel = None;
        }
        match outcome {
            SolveOutcome::Found(path) => {
                self.vcf_status = format_vcf_status("VCF found", Some(path.len()), elapsed);
                self.vcf_replay_len = path.len();
                self.vcf_solution = path;
            }
            SolveOutcome::TacticalFound(path) => {
                self.vcf_status =
                    format_vcf_status("VCF tactical found", Some(path.len()), elapsed);
                self.vcf_replay_len = path.len();
                self.vcf_solution = path;
            }
            SolveOutcome::NotFound => {
                self.vcf_status = format_vcf_status("VCF not found", None, elapsed);
                self.vcf_solution.clear();
                self.vcf_replay_len = 0;
            }
            SolveOutcome::Incomplete => {
                self.vcf_status = format_vcf_status("VCF search incomplete", None, elapsed);
                self.vcf_solution.clear();
                self.vcf_replay_len = 0;
            }
            SolveOutcome::Timeout => {
                self.vcf_status = format_vcf_status("VCF search timed out", None, elapsed);
                self.vcf_solution.clear();
                self.vcf_replay_len = 0;
            }
            SolveOutcome::Cancelled => {
                self.vcf_status = format_vcf_status("VCF search cancelled", None, elapsed);
                self.vcf_solution.clear();
                self.vcf_replay_len = 0;
            }
        }
    }

    fn finish_vct_search(&mut self, outcome: SolveOutcome) {
        self.vcf_solving = false;
        self.vcf_solving_threats = false;
        self.vcf_solving_vct = false;
        self.vcf_cancel_requested = false;
        let elapsed = self.finish_vcf_timer();
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.vcf_cancel = None;
        }

        self.vcf_threats.clear();
        match outcome {
            SolveOutcome::Found(path) => {
                self.vcf_status = format_vcf_status("VCT found", Some(path.len()), elapsed);
                self.vcf_replay_len = path.len();
                self.vcf_solution = path;
            }
            SolveOutcome::TacticalFound(path) => {
                self.vcf_status =
                    format_vcf_status("VCT tactical found", Some(path.len()), elapsed);
                self.vcf_replay_len = path.len();
                self.vcf_solution = path;
            }
            SolveOutcome::NotFound => {
                self.vcf_status = format_vcf_status("VCT not found", None, elapsed);
                self.vcf_solution.clear();
                self.vcf_replay_len = 0;
            }
            SolveOutcome::Incomplete => {
                self.vcf_status = format_vcf_status("VCT search incomplete", None, elapsed);
                self.vcf_solution.clear();
                self.vcf_replay_len = 0;
            }
            SolveOutcome::Timeout => {
                self.vcf_status = format_vcf_status("VCT search timed out", None, elapsed);
                self.vcf_solution.clear();
                self.vcf_replay_len = 0;
            }
            SolveOutcome::Cancelled => {
                self.vcf_status = format_vcf_status("VCT search cancelled", None, elapsed);
                self.vcf_solution.clear();
                self.vcf_replay_len = 0;
            }
        }
    }
}

fn format_vcf_status(prefix: &str, moves: Option<usize>, elapsed: Option<f64>) -> String {
    let mut status = if let Some(moves) = moves {
        format!("{prefix}: {moves} moves")
    } else {
        prefix.to_string()
    };

    if let Some(elapsed) = elapsed {
        status.push_str(&format!(" / {:.2}s", elapsed));
    }

    status
}

impl RenjuApp {
    #[inline(always)]
    pub fn get_comment_id(&self, idx: usize) -> u32 {
        if let Some(map) = &self.node_texts {
            if idx < map.len() {
                let texts = map[idx];
                let id = texts & 0xFFFF;
                if id != 0xFFFF {
                    return id;
                }
            }
        }
        NO_TEXT
    }

    #[inline(always)]
    pub fn get_text_id(&self, idx: usize) -> u32 {
        if let Some(map) = &self.node_texts {
            if idx < map.len() {
                let texts = map[idx];
                let id = (texts >> 16) & 0xFFFF;
                if id != 0xFFFF {
                    return id;
                }
            }
        }
        NO_TEXT
    }

    #[inline(always)]
    pub fn set_comment_id(&mut self, idx: usize, id: u32) {
        if let Some(map) = &mut self.node_texts {
            if idx >= map.len() {
                map.resize(idx + 1, 0xFFFF | (0xFFFF << 16));
            }
            let cid = std::cmp::min(id, 0xFFFF);
            map[idx] = (map[idx] & 0xFFFF0000) | cid;
        }
    }

    #[inline(always)]
    pub fn set_text_id(&mut self, idx: usize, id: u32) {
        if let Some(map) = &mut self.node_texts {
            // idx 縺後・縺ｿ蜃ｺ縺吝ｴ蜷医・鬆伜沺繧呈僑蠑ｵ縺吶ｋ
            if idx >= map.len() {
                map.resize(idx + 1, 0xFFFF | (0xFFFF << 16));
            }
            let tid = std::cmp::min(id, 0xFFFF);
            map[idx] = (map[idx] & 0x0000FFFF) | (tid << 16);
        }
    }

    /// Decode comment text.
    pub fn decode_comment(&self, idx: usize) -> Option<String> {
        let cid = self.get_comment_id(idx);
        if cid == NO_TEXT {
            return None;
        }
        if let Some(pool) = &self.text_pool {
            let enc = self.settings.text_encoding.to_encoding_rs();
            return pool.get(cid).map(|b| enc.decode(b).0.into_owned());
        }
        None
    }

    /// Decode board text.
    pub fn decode_text(&self, idx: usize) -> Option<String> {
        let tid = self.get_text_id(idx);
        if tid == NO_TEXT {
            return None;
        }
        if let Some(pool) = &self.text_pool {
            let enc = self.settings.text_encoding.to_encoding_rs();
            return pool.get(tid).map(|b| enc.decode(b).0.into_owned());
        }
        None
    }

    /// Count nodes in the subtree.
    pub fn get_subtree_size(&self, idx: usize) -> usize {
        if let Some(nodes) = &self.lib_nodes {
            let mut cache = self.subtree_cache.borrow_mut();
            if let Some(&c) = cache.get(&idx) {
                return c;
            }

            let mut count = 0;
            // 繧ｹ繧ｿ繝・け繧剃ｽｿ逕ｨ縺励※蟄仙ｭｫ繝弱・繝峨ｒ謨ｰ縺医ｋ
            let mut stack = vec![nodes[idx].child];
            while let Some(curr) = stack.pop() {
                if curr != NO_NODE {
                    count += 1;
                    stack.push(nodes[curr as usize].sibling);
                    stack.push(nodes[curr as usize].child);
                }
            }
            cache.insert(idx, count);
            count
        } else {
            0
        }
    }
}

impl eframe::App for RenjuApp {
    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        self.settings.save();
    }

    /// Main update loop.
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(target_arch = "wasm32")]
        {
            let mut loaded_data = None;
            if let Ok(mut lock) = self.wasm_dropped_file.lock() {
                if lock.is_some() {
                    loaded_data = lock.take();
                }
            }
            if let Some((file_name, data)) = loaded_data {
                let is_db = file_name.ends_with(".db");
                self.process_loaded_data(&data, file_name, is_db);
            }
        }

        #[cfg(target_os = "android")]
        self.process_android_file_events(ctx);

        let mut screenshot = None;
        for event in ctx.input(|i| i.events.clone()) {
            if let egui::Event::Screenshot { image, .. } = event {
                screenshot = Some(image);
            }
        }

        if let Some(img) = screenshot {
            let mut is_png = false;
            let mut is_gif = false;
            let mut target_rect = egui::Rect::NOTHING;
            let mut current_move = 0;

            if let ExportState::CaptureRequested(rect) = self.export_state {
                is_png = true;
                target_rect = rect;
            } else if let ExportState::GifCaptureRequested(rect, cm) = self.export_state {
                is_gif = true;
                target_rect = rect;
                current_move = cm;
            }

            if is_png || is_gif {
                let ppp = ctx.pixels_per_point();
                let physical_rect = egui::Rect::from_min_max(
                    (target_rect.min.to_vec2() * ppp).to_pos2(),
                    (target_rect.max.to_vec2() * ppp).to_pos2(),
                );
                let min_x = physical_rect.min.x.round().max(0.0) as usize;
                let min_y = physical_rect.min.y.round().max(0.0) as usize;
                let max_x = physical_rect.max.x.round().min(img.size[0] as f32) as usize;
                let max_y = physical_rect.max.y.round().min(img.size[1] as f32) as usize;

                let width = max_x.saturating_sub(min_x);
                let height = max_y.saturating_sub(min_y);

                if width > 0 && height > 0 {
                    let mut cropped = Vec::with_capacity(width * height * 4);
                    for y in min_y..max_y {
                        for x in min_x..max_x {
                            let pixel = img.pixels[y * img.size[0] + x];
                            cropped.push(pixel.r());
                            cropped.push(pixel.g());
                            cropped.push(pixel.b());
                            cropped.push(pixel.a());
                        }
                    }

                    if let Some(img_buf) =
                        image::RgbaImage::from_raw(width as u32, height as u32, cropped)
                    {
                        if is_png {
                            self.export_state = ExportState::Idle;
                            #[cfg(all(
                                not(target_arch = "wasm32"),
                                not(target_os = "android"),
                                not(target_os = "ios")
                            ))]
                            {
                                if let Some(path) = rfd::FileDialog::new()
                                    .add_filter("PNG Image", &["png"])
                                    .set_file_name("renju_board.png")
                                    .save_file()
                                {
                                    let _ = img_buf.save(path);
                                }
                            }
                            #[cfg(target_os = "android")]
                            {
                                self.vcf_status =
                                    "Android native image export is not implemented yet"
                                        .to_string();
                            }
                            #[cfg(target_os = "ios")]
                            {
                                self.vcf_status =
                                    "iOS image export is not implemented yet".to_string();
                            }
                            #[cfg(target_arch = "wasm32")]
                            {
                                let mut png_data = Vec::new();
                                let mut cursor = std::io::Cursor::new(&mut png_data);
                                if img_buf
                                    .write_to(&mut cursor, image::ImageOutputFormat::Png)
                                    .is_ok()
                                {
                                    wasm_bindgen_futures::spawn_local(async move {
                                        if let Some(handle) = rfd::AsyncFileDialog::new()
                                            .add_filter("PNG Image", &["png"])
                                            .set_file_name("renju_board.png")
                                            .save_file()
                                            .await
                                        {
                                            let _ = handle.write(&png_data).await;
                                        }
                                    });
                                }
                            }
                        } else if is_gif {
                            let img_to_push = if let Some(first_frame) = self.gif_frames.first() {
                                let expected_width = first_frame.width();
                                let expected_height = first_frame.height();

                                // 1繝斐け繧ｻ繝ｫ縺ｧ繧ゅぜ繝ｬ縺ｦ縺・◆繧峨∵怙蛻昴・繝輔Ξ繝ｼ繝縺ｮ繧ｵ繧､繧ｺ縺ｫ蜷医ｏ縺帙※蠑ｷ蛻ｶ繝ｪ繧ｵ繧､繧ｺ
                                if img_buf.width() != expected_width
                                    || img_buf.height() != expected_height
                                {
                                    image::imageops::resize(
                                        &img_buf,
                                        expected_width,
                                        expected_height,
                                        image::imageops::FilterType::Nearest,
                                    )
                                } else {
                                    img_buf
                                }
                            } else {
                                img_buf
                            };

                            self.gif_frames.push(img_to_push);
                            let target_end_move =
                                self.gif_settings.end_move.min(self.board.history.len());
                            if current_move < target_end_move {
                                self.export_state =
                                    ExportState::GifWaitCapture(target_rect, current_move + 1);
                                ctx.request_repaint();
                            } else {
                                self.export_state = ExportState::Idle;
                                let frames = std::mem::take(&mut self.gif_frames);
                                let settings = self.gif_settings.clone();

                                #[cfg(all(
                                    not(target_arch = "wasm32"),
                                    not(target_os = "android"),
                                    not(target_os = "ios")
                                ))]
                                {
                                    if let Some(path) = rfd::FileDialog::new()
                                        .add_filter("GIF Image", &["gif"])
                                        .set_file_name("renju_board.gif")
                                        .save_file()
                                    {
                                        std::thread::spawn(move || {
                                            if let Ok(mut file) = std::fs::File::create(path) {
                                                let mut encoder =
                                                    image::codecs::gif::GifEncoder::new(&mut file);
                                                let _ = encoder.set_repeat(
                                                    image::codecs::gif::Repeat::Infinite,
                                                );
                                                let total_frames = frames.len();
                                                for (i, frame_img) in frames.into_iter().enumerate()
                                                {
                                                    let mut delay_ms =
                                                        (settings.frame_delay_sec * 1000.0) as u32;
                                                    if i == 0 {
                                                        delay_ms = (settings.first_frame_delay_sec
                                                            * 1000.0)
                                                            as u32;
                                                    }
                                                    if i == total_frames - 1 {
                                                        delay_ms = (settings.last_frame_delay_sec
                                                            * 1000.0)
                                                            as u32;
                                                    }
                                                    let frame = image::Frame::from_parts(
                                                        frame_img,
                                                        0,
                                                        0,
                                                        image::Delay::from_numer_denom_ms(
                                                            delay_ms, 1,
                                                        ),
                                                    );
                                                    let _ = encoder.encode_frame(frame);
                                                }
                                            }
                                        });
                                    }
                                }
                                #[cfg(target_os = "android")]
                                {
                                    let _ = (&frames, &settings);
                                    self.vcf_status =
                                        "Android native GIF export is not implemented yet"
                                            .to_string();
                                }
                                #[cfg(target_os = "ios")]
                                {
                                    let _ = (&frames, &settings);
                                    self.vcf_status =
                                        "iOS GIF export is not implemented yet".to_string();
                                }
                                #[cfg(target_arch = "wasm32")]
                                {
                                    wasm_bindgen_futures::spawn_local(async move {
                                        let mut encoded_gif = Vec::new();
                                        {
                                            let mut encoder = image::codecs::gif::GifEncoder::new(
                                                &mut encoded_gif,
                                            );
                                            let _ = encoder
                                                .set_repeat(image::codecs::gif::Repeat::Infinite);
                                            let total_frames = frames.len();
                                            for (i, frame_img) in frames.into_iter().enumerate() {
                                                let mut delay_ms =
                                                    (settings.frame_delay_sec * 1000.0) as u32;
                                                if i == 0 {
                                                    delay_ms = (settings.first_frame_delay_sec
                                                        * 1000.0)
                                                        as u32;
                                                }
                                                if i == total_frames - 1 {
                                                    delay_ms = (settings.last_frame_delay_sec
                                                        * 1000.0)
                                                        as u32;
                                                }
                                                let frame = image::Frame::from_parts(
                                                    frame_img,
                                                    0,
                                                    0,
                                                    image::Delay::from_numer_denom_ms(delay_ms, 1),
                                                );
                                                let _ = encoder.encode_frame(frame);
                                            }
                                        }
                                        if let Some(handle) = rfd::AsyncFileDialog::new()
                                            .add_filter("GIF Image", &["gif"])
                                            .set_file_name("renju_board.gif")
                                            .save_file()
                                            .await
                                        {
                                            let _ = handle.write(&encoded_gif).await;
                                        }
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        let tr = self.settings.tr();
        let screen_rect = ctx.screen_rect();
        let is_vertical = screen_rect.width() < screen_rect.height() * 0.8;

        crate::ui::dialogs::draw_delete_confirm_dialog(self, ctx, &tr);
        crate::ui::dialogs::draw_new_game_confirm_dialog(self, ctx, &tr);
        crate::ui::dialogs::draw_copy_notification(self, ctx);
        #[cfg(target_os = "windows")]
        crate::ui::dialogs::draw_vct_log_dialog(self, ctx);

        crate::ui::dialogs::draw_text_edit_dialog(self, ctx);

        self.poll_vcf_search(ctx);

        if self.node_to_delete.is_none() && self.editing_coord.is_none() {
            if ctx.input(|i| i.key_pressed(egui::Key::Delete)) {
                if let Some(idx) = self.get_current_node() {
                    if idx > 0 {
                        self.node_to_delete = Some(idx);
                    }
                }
            }
        }

        crate::ui::toolbar::draw_toolbar(self, ctx, &tr);

        crate::ui::panel::draw_comment_panel(self, ctx, &tr, is_vertical, screen_rect);

        egui::CentralPanel::default().show(ctx, |ui| {
            crate::ui::board::draw_main_board(self, ctx, ui);
        });
    }
}

impl Drop for RenjuApp {
    /// Save settings on shutdown.
    fn drop(&mut self) {
        self.settings.save();
    }
}

#[cfg(test)]
mod placement_tests {
    use super::*;
    use crate::board::{Player, SIZE};

    #[test]
    fn free_stones_affect_forbidden_points_and_cache() {
        // Avoid saving test settings through the application's Drop implementation.
        let mut app =
            std::mem::ManuallyDrop::new(RenjuApp::new_with_settings(setting::Settings::default()));
        app.settings.free_placement = true;
        app.board.begin_free_placement();
        for (x, y) in [(6, 7), (8, 7), (7, 6), (7, 8)] {
            app.board.toggle_setup_stone(x, y, Player::Black);
        }
        let center = 7 * SIZE + 7;
        assert!(app.board.history.is_empty());
        assert!(app.cached_forbidden_points().contains(&center));
        app.board.toggle_setup_stone(9, 7, Player::White);
        assert!(!app.cached_forbidden_points().contains(&center));
        app.board.toggle_setup_stone(9, 7, Player::White);
        assert!(app.cached_forbidden_points().contains(&center));
        app.board.begin_sequence(Player::White);
        app.settings.free_placement = false;
        app.clear_vcf_solution();
        assert!(app.cached_forbidden_points().is_empty());
        assert_eq!(
            app.search_side_to_move(&app.current_vcf_board()),
            Ok(crate::vcf::solver::WHITE)
        );
        app.play_move(0, 0);
        assert_eq!(app.board.grid[0], Some(Player::White));
        assert_eq!(app.board.history, vec![0]);
        assert!(app.cached_forbidden_points().contains(&center));
        assert_eq!(app.settings.stone_number(1), Some(1));
        assert_eq!(app.get_current_node(), None);
        app.reset_to_root();
        assert_eq!(app.board.grid, app.board.setup_grid);
        assert!(app.step_forward());
        assert_eq!(app.board.grid[0], Some(Player::White));
        assert!(!app.step_forward());
    }
}
