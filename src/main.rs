#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
#[cfg(target_arch = "wasm32")]
use serde::{Deserialize, Serialize};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::closure::Closure;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;
#[cfg(target_arch = "wasm32")]
use std::sync::{Arc, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

mod board; 
mod core;
mod ui;
mod io;
mod setting; 
mod lang; 
mod exportkifu;
mod vcf;

use board::Board;
use rustc_hash::FxHashMap;
use crate::core::node::{RenLibNode, TextPool, NO_NODE, NO_TEXT};
use crate::core::hash_board::Board as HashBoard;
use crate::io::parser::RenLibParser;
use crate::io::writer::RenLibWriter;
use crate::vcf::model::BOARD_CELLS;
use crate::vcf::solver::{BLACK, Move as VcfMove, SearchMode, SolveOutcome, Solver, side_to_move};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::Receiver;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(inline_js = r#"
export function isVcfWorkerContext() {
    return globalThis.__RENJU_VCF_WORKER === true;
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
        worker.terminate();
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

    #[wasm_bindgen(catch, js_name = spawnVcfWorker)]
    fn spawn_vcf_worker(
        request_json: &str,
        callback: &js_sys::Function,
    ) -> Result<web_sys::Worker, JsValue>;
}

#[cfg(target_arch = "wasm32")]
#[derive(Serialize, Deserialize)]
struct VcfWorkerRequest {
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
    NotFound,
    Timeout,
    Cancelled,
    Error { message: String },
}

#[cfg(target_arch = "wasm32")]
impl From<SolveOutcome> for VcfWorkerResponse {
    fn from(outcome: SolveOutcome) -> Self {
        match outcome {
            SolveOutcome::Found(moves) => Self::Found { moves },
            SolveOutcome::NotFound => Self::NotFound,
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
            let mode = if request.mode == "long" {
                SearchMode::Long
            } else {
                SearchMode::Shortest
            };
            let outcome = Solver::solve(
                board,
                request.turn,
                request.depth,
                std::time::Duration::from_secs(request.timeout_secs.max(1)),
                mode,
            );
            VcfWorkerResponse::from(outcome)
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
// フォント設定
// ==========================================

/// カスタムフォントを読み込み、eguiコンテキストに登録する
pub fn setup_custom_fonts(ctx: &egui::Context, _path: &str) {
    let mut fonts = egui::FontDefinitions::default();
    
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Ok(font_data) = std::fs::read(_path) {
            fonts.font_data.insert(
                "custom_font".to_owned(),
                Arc::new(egui::FontData::from_owned(font_data)),
            );
            fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "custom_font".to_owned());
            fonts.families.entry(egui::FontFamily::Monospace).or_default().insert(0, "custom_font".to_owned());
            ctx.set_fonts(fonts);
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        // 各フォントデータの読み込み
        let font_jp = include_bytes!("../Renjulibviewer_JP.ttf");
        let font_sc = include_bytes!("../Renjulibviewer_SC.ttf");
        let font_tc = include_bytes!("../Renjulibviewer_TC.ttf");
        let font_kr = include_bytes!("../Renjulibviewer_KR.ttf");

        fonts.font_data.insert("font_jp".to_owned(), Arc::new(egui::FontData::from_static(font_jp)));
        fonts.font_data.insert("font_sc".to_owned(), Arc::new(egui::FontData::from_static(font_sc)));
        fonts.font_data.insert("font_tc".to_owned(), Arc::new(egui::FontData::from_static(font_tc)));
        fonts.font_data.insert("font_kr".to_owned(), Arc::new(egui::FontData::from_static(font_kr)));

        // 優先順位の設定: JP -> SC -> TC -> KR
        let families = [
            egui::FontFamily::Proportional,
            egui::FontFamily::Monospace,
        ];

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


/// アプリケーションアイコンを読み込む
#[cfg(not(target_arch = "wasm32"))]
fn load_icon() -> egui::IconData {
    let icon_data = include_bytes!("../app_icon.png"); 
    let image = image::load_from_memory(icon_data)
        .expect("Failed to load icon")
        .into_rgba8();
    
    let (width, height) = image.dimensions();
    let rgba = image.into_raw();
    
    egui::IconData { rgba, width, height }
}

// ==========================================
// エントリポイント (起動処理)
// ==========================================

/// ネイティブ環境でのアプリケーション起動処理
#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
    // コマンドライン引数を取得
    let mut args = std::env::args().skip(1); // 0 番目は実行ファイル自身
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

            // アプリ本体を生成
            let mut app = RenjuApp::new_with_settings(settings);

            // 起動時にファイルが渡されていれば即読込
            if let Some(path_str) = maybe_path {
                app.load_file(std::path::Path::new(&path_str));
            }

            Ok(Box::new(app))
        }),
    )
}

/// WASM環境でのアプリケーション起動処理
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
        let canvas: web_sys::HtmlCanvasElement = canvas.dyn_into::<web_sys::HtmlCanvasElement>().map_err(|_| ()).unwrap();

        eframe::WebRunner::new()
            .start(
                canvas,
                web_options,
                Box::new(|cc| {
                    let settings = setting::Settings::load();
                    setup_custom_fonts(&cc.egui_ctx, &settings.font_path);
                    
                    // スマホでの入力時ズーム対策: フォントサイズを16px相当以上に設定
                    let mut style = (*cc.egui_ctx.style()).clone();
                    style.text_styles = [
                        (egui::TextStyle::Body, egui::FontId::new(16.0, egui::FontFamily::Proportional)),
                        (egui::TextStyle::Button, egui::FontId::new(16.0, egui::FontFamily::Proportional)),
                        (egui::TextStyle::Monospace, egui::FontId::new(16.0, egui::FontFamily::Monospace)),
                        (egui::TextStyle::Heading, egui::FontId::new(18.0, egui::FontFamily::Proportional)),
                    ].into();
                    cc.egui_ctx.set_style(style);

                    // ブラウザ側に「十分な大きさ」と認識させるためズームファクタを調整
                    cc.egui_ctx.set_zoom_factor(1.1);

                    Ok(Box::new(RenjuApp::new_with_settings(settings)))
                }),
            )
            .await
            .expect("failed to start eframe");
    });
}

// ==========================================
// アプリケーションの状態管理とメインロジック
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

// ExportState 定義はそのまま
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
    pub copy_notification: Option<(String, web_time::Instant)>,
    pub vcf_solution: Vec<VcfMove>,
    pub vcf_status: String,
    pub vcf_solving: bool,
    pub forbidden_points_board: [u8; BOARD_CELLS],
    pub forbidden_points: Vec<usize>,
    pub forbidden_points_valid: bool,
    #[cfg(not(target_arch = "wasm32"))]
    pub vcf_rx: Option<Receiver<SolveOutcome>>,
    #[cfg(target_arch = "wasm32")]
    pub vcf_worker: Option<web_sys::Worker>,
    #[cfg(target_arch = "wasm32")]
    pub vcf_worker_callback: Option<Closure<dyn FnMut(JsValue)>>,
    #[cfg(target_arch = "wasm32")]
    pub vcf_worker_result: Arc<Mutex<Option<String>>>,
    #[cfg(target_arch = "wasm32")]
    pub wasm_dropped_file: Arc<Mutex<Option<(String, Vec<u8>)>>>,
}


impl RenjuApp {
    /// 設定値を使用してアプリケーション状態を初期化する
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
            copy_notification: None,
            vcf_solution: Vec::new(),
            vcf_status: String::new(),
            vcf_solving: false,
            forbidden_points_board: [0; BOARD_CELLS],
            forbidden_points: Vec::new(),
            forbidden_points_valid: false,
            #[cfg(not(target_arch = "wasm32"))]
            vcf_rx: None,
            #[cfg(target_arch = "wasm32")]
            vcf_worker: None,
            #[cfg(target_arch = "wasm32")]
            vcf_worker_callback: None,
            #[cfg(target_arch = "wasm32")]
            vcf_worker_result: Arc::new(Mutex::new(None)),
            #[cfg(target_arch = "wasm32")]
            wasm_dropped_file: Arc::new(Mutex::new(None)),
        }
    }

    /// 盤面とツリーを完全に初期化し、新規作成状態にする
    pub fn new_game(&mut self) {
        self.board = Board::new();
        self.lib_nodes = Some(vec![RenLibNode::new(-1, -1, NO_NODE, NO_NODE, NO_NODE, 0, 0)]);
        
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
        
        // UI・操作状態のクリア
        self.node_to_delete = None;
        self.editing_coord = None;
        self.editing_text.clear();
        self.ad_hoc_labels.clear();
        self.subtree_cache.borrow_mut().clear();
        self.export_state = ExportState::Idle;
        self.show_new_game_confirm = false;
        self.clear_vcf_solution();
    }

    /// 読み込まれたバイトデータからパースを行い、アプリケーション状態を更新する
    #[cfg(target_arch = "wasm32")]
    fn process_loaded_data(&mut self, data: &[u8], file_name: String, is_db: bool) {
        let start_time = web_time::Instant::now();
        let encoding = self.settings.text_encoding.to_encoding_rs();
        
        let mut parser = RenLibParser::new(self.settings.max_nodes);
        parser.set_encoding(encoding);
        let mut parse_board = HashBoard::new();
        
        let result = if is_db { parser.parse_db(data) } else { parser.parse_all(data, &mut parse_board) };
        
        if result.is_ok() {
            self.current_file = Some(file_name);
            self.lib_nodes = Some(parser.nodes);
            self.hash_table = Some(parser.hash_table);
            self.node_texts = Some(parser.node_texts);
            self.text_pool = Some(parser.text_pool); 
            self.subtree_cache.borrow_mut().clear(); 
            self.current_node_idx = Some(0);
            self.board = Board::new();
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

    /// ファイルパスから拡張子を自動判定して読み込む
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_file(&mut self, path: &std::path::Path) {
        let is_db = matches!(path.extension().and_then(|e| e.to_str()), Some("db"));
        self.process_loaded_file(path, is_db);
    }

    /// ファイルパスからデータを読み込み、パースを行う
    #[cfg(not(target_arch = "wasm32"))]
    pub fn process_loaded_file(&mut self, path: &std::path::Path, is_db: bool) {
        let start_time = web_time::Instant::now();
        let encoding = self.settings.text_encoding.to_encoding_rs();
        
        let mut parser = RenLibParser::new(self.settings.max_nodes);
        parser.set_encoding(encoding);
        let mut parse_board = HashBoard::new();
        
        let file_name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        
        // ファイルをオープンしてストリームとして処理（メモリに全展開しない）
        let result = if let Ok(file) = std::fs::File::open(path) {
            if is_db {
                parser.parse_db_from_reader(file)
            } else {
                // .lib はファイルサイズが小さく parse_all がスライス前提のため読み込む
                if let Ok(data) = std::fs::read(path) {
                    parser.parse_all(&data, &mut parse_board)
                } else {
                    Err(std::io::Error::new(std::io::ErrorKind::NotFound, "File read error"))
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

    /// 現在の盤面状態に対応するノードのインデックスを取得する
    fn get_current_node(&self) -> Option<usize> {
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

    /// 盤面を初期状態（ルート）に戻す
    fn reset_to_root(&mut self) {
        if self.is_db_mode {
            self.board.undo_all();
        } else if let Some(nodes) = &self.lib_nodes {
            // ルートに到達するまで一手ずつ戻る
            while let Some(idx) = self.current_node_idx {
                if idx == 0 { break; }
                let node = &nodes[idx];
                if node.x() >= 0 && node.y() >= 0 { self.board.undo(); }
                
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

    /// 指定された座標に石を打ち、必要に応じて新しいノードをツリーに追加する
    fn play_move(&mut self, ux: usize, uy: usize) {
        let before_len = self.board.history.len();
        if self.is_db_mode {
            self.board.place_stone(ux, uy);
        } else if let Some(nodes) = &mut self.lib_nodes {
            if let Some(idx) = self.current_node_idx {
                let mut child_idx = nodes[idx].child;
                let mut matched = false;
                
                // 同じ座標の既存の子ノードがないか探す
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
                            ux as i8, uy as i8, idx as u32, NO_NODE, nodes[idx].child, new_hash, nodes[idx].depth() + 1
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

    /// 現在選択されている分岐とその子孫をすべて削除する
    fn delete_current_branch(&mut self) {
        if let Some(idx) = self.node_to_delete {
            let is_current = self.get_current_node() == Some(idx);
            let before_len = self.board.history.len();
            
            if let Some(nodes) = &mut self.lib_nodes {
                if idx > 0 && idx < nodes.len() {
                    let mut subtree_indices = Vec::new();
                    
                    // スタックを使用して子孫ノードを再帰的に収集する
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

    /// 履歴または単一の分岐に基づいて一手進める
    fn step_forward(&mut self) -> bool {
        // 1. ユーザーの入力履歴（future_history）があればそれを復元する
        if let Some(&next_pos) = self.board.future_history.last() {
            self.board.redo();
            if !self.is_db_mode {
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
        
        // 2. 履歴が無く、かつ登録されている分岐が1つだけならそこへ自動的に進む
        if self.is_db_mode {
            // DBモード: 盤面の全空きマスに仮置きし、ハッシュ辞書を検索して分岐を数える
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
                            
                            // データベースに存在する局面ハッシュなら分岐としてカウント
                            if ht.contains_key(&h) {
                                valid_moves.push((x, y));
                            }
                        }
                    }
                }
            }
            
            // 分岐がちょうど1つの場合のみ自動で手を進める
            if valid_moves.len() == 1 {
                self.board.place_stone(valid_moves[0].0, valid_moves[0].1);
                self.clear_vcf_solution();
                return true;
            }
        } else {
            // LIBモード: 既存のツリーポインタによる判定
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

    // 現在のノードに至るまでの直列な履歴と、現在のノード以下の全分岐を抽出する
    /// 現在のブランチに関連するノードのみを抽出したツリーを構築する
    fn get_branch_nodes(&self) -> Option<Vec<RenLibNode>> {
        let current_idx = self.get_current_node()?;
        let original = self.lib_nodes.as_ref()?;
        let mut nodes = original.clone();
        
        // 全てのノードを一旦「削除状態 (-2, -2)」にする
        for node in nodes.iter_mut() {
            node.set_x(-2);
            node.set_y(-2);
        }

        // 1. 現在のノード以下のすべての子孫を復活させる
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

        // 2. 現在のノードからRoot(初手)までの一本道を復活させる
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

    // 「名前を付けて保存」「分岐保存」のダイアログ処理
    /// ファイル保存ダイアログを表示し、データを保存する
    fn save_file_dialog(&mut self, branch_only: bool) {
        let nodes_to_save = if branch_only {
            self.get_branch_nodes()
        } else {
            self.lib_nodes.clone()
        };

        if let (Some(nodes), Some(pool), Some(node_texts)) = (nodes_to_save, &self.text_pool, &self.node_texts) {
            #[cfg(not(target_arch = "wasm32"))]
            {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("RenLib files", &["lib"])
                    .add_filter("Yixin database", &["db"])
                    .save_file()
                {
                    let is_db = path.extension().and_then(|s| s.to_str()) == Some("db");
                    let encoding = self.settings.text_encoding.to_encoding_rs();
                    let mut writer = RenLibWriter::new(encoding);
                    let data = if is_db { writer.save_db(&nodes, node_texts, pool) } else { writer.save(&nodes, node_texts, pool) };
                    if std::fs::write(&path, data).is_ok() && !branch_only {
                        // 全体保存の場合は現在のファイルパスを更新する
                        self.current_filepath = Some(path.clone());
                        self.current_file = Some(path.file_name().and_then(|s| s.to_str()).unwrap_or("Unknown").to_string());
                    }
                }
            }
            #[cfg(target_arch = "wasm32")]
            {
                let encoding = self.settings.text_encoding.to_encoding_rs();
                let mut writer = RenLibWriter::new(encoding);
                let data = writer.save(&nodes, node_texts, pool); 
                wasm_bindgen_futures::spawn_local(async move {
                    if let Some(handle) = rfd::AsyncFileDialog::new().add_filter("RenLib files", &["lib"]).set_file_name("saved_game.lib").save_file().await {
                        let _ = handle.write(&data).await;
                    }
                });
            }
        }
    }

    // 上書き保存処理（新規作成等でパスがない場合はダイアログへ）
    /// 現在開いているファイルに上書き保存する
    fn overwrite_save(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let (Some(path), Some(nodes), Some(pool), Some(node_texts)) = (&self.current_filepath, &self.lib_nodes, &self.text_pool, &self.node_texts) {
                let is_db = path.extension().and_then(|s| s.to_str()) == Some("db");
                let encoding = self.settings.text_encoding.to_encoding_rs();
                let mut writer = RenLibWriter::new(encoding);
                let data = if is_db { writer.save_db(nodes, node_texts, pool) } else { writer.save(nodes, node_texts, pool) };
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

    /// コピー通知を表示する
    pub fn trigger_copy_notification(&mut self) {
        let tr = self.settings.tr();
        self.copy_notification = Some((tr.copied_to_clipboard.to_string(), web_time::Instant::now()));
    }

    pub fn clear_vcf_solution(&mut self) {
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
        }

        self.vcf_solution.clear();
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

    pub fn cached_forbidden_points(&mut self) -> &[usize] {
        let board = self.current_vcf_board();
        if !self.forbidden_points_valid || self.forbidden_points_board != board {
            self.forbidden_points = Solver::black_forbidden_points(board);
            self.forbidden_points_board = board;
            self.forbidden_points_valid = true;
        }
        &self.forbidden_points
    }

    pub fn start_vcf_search(&mut self) {
        if self.vcf_solving {
            return;
        }
        let mode = match self.settings.vcf_mode {
            setting::VcfMode::Shortest => SearchMode::Shortest,
            setting::VcfMode::Long => SearchMode::Long,
        };
        let board = self.current_vcf_board();
        let turn = match side_to_move(&board) {
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
        self.vcf_solving = true;
        self.vcf_status = format!(
            "{} {} / depth {} / {}s",
            if turn == BLACK { "Black" } else { "White" },
            if mode == SearchMode::Long { "VCF long" } else { "VCF shortest" },
            depth,
            timeout_secs
        );

        #[cfg(not(target_arch = "wasm32"))]
        {
            let timeout = std::time::Duration::from_secs(timeout_secs);
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(Solver::solve(board, turn, depth, timeout, mode));
            });
            self.vcf_rx = Some(rx);
        }

        #[cfg(target_arch = "wasm32")]
        {
            let request = VcfWorkerRequest {
                board: board.to_vec(),
                turn,
                depth,
                timeout_secs,
                mode: if mode == SearchMode::Long {
                    "long".to_string()
                } else {
                    "shortest".to_string()
                },
            };

            let request_json = match serde_json::to_string(&request) {
                Ok(json) => json,
                Err(error) => {
                    self.vcf_solving = false;
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
                    self.vcf_status = format!("VCF worker start failed: {error:?}");
                }
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn poll_vcf_search(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.vcf_rx {
            match rx.try_recv() {
                Ok(outcome) => {
                    self.vcf_rx = None;
                    self.finish_vcf_search(outcome);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(80));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.vcf_rx = None;
                    self.vcf_solving = false;
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
            self.vcf_worker = None;
            self.vcf_worker_callback = None;
            match serde_json::from_str::<VcfWorkerResponse>(&response_json) {
                Ok(VcfWorkerResponse::Found { moves }) => {
                    self.finish_vcf_search(SolveOutcome::Found(moves));
                }
                Ok(VcfWorkerResponse::NotFound) => {
                    self.finish_vcf_search(SolveOutcome::NotFound);
                }
                Ok(VcfWorkerResponse::Timeout) => {
                    self.finish_vcf_search(SolveOutcome::Timeout);
                }
                Ok(VcfWorkerResponse::Cancelled) => {
                    self.finish_vcf_search(SolveOutcome::Cancelled);
                }
                Ok(VcfWorkerResponse::Error { message }) => {
                    self.vcf_solving = false;
                    self.vcf_solution.clear();
                    self.vcf_status = format!("VCF worker error: {message}");
                }
                Err(error) => {
                    self.vcf_solving = false;
                    self.vcf_solution.clear();
                    self.vcf_status = format!("VCF worker response failed: {error}");
                }
            }
            ctx.request_repaint();
        } else if self.vcf_solving {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }
    }

    fn finish_vcf_search(&mut self, outcome: SolveOutcome) {
        self.vcf_solving = false;
        match outcome {
            SolveOutcome::Found(path) => {
                self.vcf_status = format!("VCF found: {} moves", path.len());
                self.vcf_solution = path;
            }
            SolveOutcome::NotFound => {
                self.vcf_status = "VCF not found".to_string();
                self.vcf_solution.clear();
            }
            SolveOutcome::Timeout => {
                self.vcf_status = "VCF search timed out".to_string();
                self.vcf_solution.clear();
            }
            SolveOutcome::Cancelled => {
                self.vcf_status = "VCF search cancelled".to_string();
                self.vcf_solution.clear();
            }
        }
    }
}

impl RenjuApp {
    #[inline(always)]
    pub fn get_comment_id(&self, idx: usize) -> u32 {
        if let Some(map) = &self.node_texts {
            if idx < map.len() {
                let texts = map[idx];
                let id = texts & 0xFFFF;
                if id != 0xFFFF { return id; }
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
                if id != 0xFFFF { return id; }
            }
        }
        NO_TEXT
    }
    
    #[inline(always)]
    pub fn set_comment_id(&mut self, idx: usize, id: u32) {
        if let Some(map) = &mut self.node_texts {
            // idx がはみ出す場合は領域を拡張する（手動で手を打った場合など）
            if idx >= map.len() { map.resize(idx + 1, 0xFFFF | (0xFFFF << 16)); }
            let cid = std::cmp::min(id, 0xFFFF);
            map[idx] = (map[idx] & 0xFFFF0000) | cid;
        }
    }
    
    #[inline(always)]
    pub fn set_text_id(&mut self, idx: usize, id: u32) {
        if let Some(map) = &mut self.node_texts {
            // idx がはみ出す場合は領域を拡張する
            if idx >= map.len() { map.resize(idx + 1, 0xFFFF | (0xFFFF << 16)); }
            let tid = std::cmp::min(id, 0xFFFF);
            map[idx] = (map[idx] & 0x0000FFFF) | (tid << 16);
        }
    }

    /// コメントIDから実際の文字列をデコードして取得する
    pub fn decode_comment(&self, idx: usize) -> Option<String> {
        let cid = self.get_comment_id(idx);
        if cid == NO_TEXT { return None; }
        if let Some(pool) = &self.text_pool {
            let enc = self.settings.text_encoding.to_encoding_rs();
            return pool.get(cid).map(|b| enc.decode(b).0.into_owned());
        }
        None
    }

    /// テキストIDから実際の文字列をデコードして取得する
    pub fn decode_text(&self, idx: usize) -> Option<String> {
        let tid = self.get_text_id(idx);
        if tid == NO_TEXT { return None; }
        if let Some(pool) = &self.text_pool {
            let enc = self.settings.text_encoding.to_encoding_rs();
            return pool.get(tid).map(|b| enc.decode(b).0.into_owned());
        }
        None
    }

    /// 指定されたノード以下の全子孫ノード数を計算する（キャッシュ利用）
    pub fn get_subtree_size(&self, idx: usize) -> usize {
        if let Some(nodes) = &self.lib_nodes {
            let mut cache = self.subtree_cache.borrow_mut();
            if let Some(&c) = cache.get(&idx) { return c; }
            
            let mut count = 0;
            // スタックを使用して子孫ノードを数える
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
        } else { 0 }
    }
}

    
    

    

    

    
    
    
    



    

    

    

    


    

    



impl eframe::App for RenjuApp {
    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        self.settings.save();
    }

    /// 毎フレームの描画および入力処理のメインループ
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        
        #[cfg(target_arch = "wasm32")]
        {
            let mut loaded_data = None;
            if let Ok(mut lock) = self.wasm_dropped_file.lock() {
                if lock.is_some() { loaded_data = lock.take(); }
            }
            if let Some((file_name, data)) = loaded_data {
                let is_db = file_name.ends_with(".db");
                self.process_loaded_data(&data, file_name, is_db);
            }
        }

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
                is_png = true; target_rect = rect;
            } else if let ExportState::GifCaptureRequested(rect, cm) = self.export_state {
                is_gif = true; target_rect = rect; current_move = cm;
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

                    if let Some(img_buf) = image::RgbaImage::from_raw(width as u32, height as u32, cropped) {
                        if is_png {
                            self.export_state = ExportState::Idle;
                            #[cfg(not(target_arch = "wasm32"))]
                            {
                                if let Some(path) = rfd::FileDialog::new().add_filter("PNG Image", &["png"]).set_file_name("renju_board.png").save_file() {
                                    let _ = img_buf.save(path);
                                }
                            }
                            #[cfg(target_arch = "wasm32")]
                            {
                                let mut png_data = Vec::new();
                                let mut cursor = std::io::Cursor::new(&mut png_data);
                                if img_buf.write_to(&mut cursor, image::ImageOutputFormat::Png).is_ok() {
                                    wasm_bindgen_futures::spawn_local(async move {
                                        if let Some(handle) = rfd::AsyncFileDialog::new().add_filter("PNG Image", &["png"]).set_file_name("renju_board.png").save_file().await {
                                            let _ = handle.write(&png_data).await;
                                        }
                                    });
                                }
                            }
                        } else if is_gif {
                            let img_to_push = if let Some(first_frame) = self.gif_frames.first() {
    let expected_width = first_frame.width();
    let expected_height = first_frame.height();
    
    // 1ピクセルでもズレていたら、最初のフレームのサイズに合わせて強制リサイズ
    if img_buf.width() != expected_width || img_buf.height() != expected_height {
        // Nearest（ニアレストネイバー法）が最も高速で、ピクセルアート的な連珠盤の劣化も少ないです
        image::imageops::resize(&img_buf, expected_width, expected_height, image::imageops::FilterType::Nearest)
    } else {
        img_buf
    }
} else {
    img_buf
};

self.gif_frames.push(img_to_push);
                            let target_end_move = self.gif_settings.end_move.min(self.board.history.len());
                            if current_move < target_end_move {
                                self.export_state = ExportState::GifWaitCapture(target_rect, current_move + 1);
                                ctx.request_repaint();
                            } else {
                                self.export_state = ExportState::Idle;
                                let frames = std::mem::take(&mut self.gif_frames);
                                let settings = self.gif_settings.clone();

                                #[cfg(not(target_arch = "wasm32"))]
                                {
                                    if let Some(path) = rfd::FileDialog::new().add_filter("GIF Image", &["gif"]).set_file_name("renju_board.gif").save_file() {
                                        std::thread::spawn(move || {
                                            if let Ok(mut file) = std::fs::File::create(path) {
                                                let mut encoder = image::codecs::gif::GifEncoder::new(&mut file);
                                                let _ = encoder.set_repeat(image::codecs::gif::Repeat::Infinite);
                                                let total_frames = frames.len();
                                                for (i, frame_img) in frames.into_iter().enumerate() {
                                                    let mut delay_ms = (settings.frame_delay_sec * 1000.0) as u32;
                                                    if i == 0 { delay_ms = (settings.first_frame_delay_sec * 1000.0) as u32; }
                                                    if i == total_frames - 1 { delay_ms = (settings.last_frame_delay_sec * 1000.0) as u32; }
                                                    let frame = image::Frame::from_parts(frame_img, 0, 0, image::Delay::from_numer_denom_ms(delay_ms, 1));
                                                    let _ = encoder.encode_frame(frame);
                                                }
                                            }
                                        });
                                    }
                                }
                                #[cfg(target_arch = "wasm32")]
                                {
                                    wasm_bindgen_futures::spawn_local(async move {
                                        let mut encoded_gif = Vec::new();
                                        {
                                            let mut encoder = image::codecs::gif::GifEncoder::new(&mut encoded_gif);
                                            let _ = encoder.set_repeat(image::codecs::gif::Repeat::Infinite);
                                            let total_frames = frames.len();
                                            for (i, frame_img) in frames.into_iter().enumerate() {
                                                let mut delay_ms = (settings.frame_delay_sec * 1000.0) as u32;
                                                if i == 0 { delay_ms = (settings.first_frame_delay_sec * 1000.0) as u32; }
                                                if i == total_frames - 1 { delay_ms = (settings.last_frame_delay_sec * 1000.0) as u32; }
                                                let frame = image::Frame::from_parts(frame_img, 0, 0, image::Delay::from_numer_denom_ms(delay_ms, 1));
                                                let _ = encoder.encode_frame(frame);
                                            }
                                        }
                                        if let Some(handle) = rfd::AsyncFileDialog::new().add_filter("GIF Image", &["gif"]).set_file_name("renju_board.gif").save_file().await {
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

        crate::ui::dialogs::draw_text_edit_dialog(self, ctx);

        self.poll_vcf_search(ctx);

        if self.node_to_delete.is_none() && self.editing_coord.is_none() {
            if ctx.input(|i| i.key_pressed(egui::Key::Delete)) {
                if let Some(idx) = self.get_current_node() {
                    if idx > 0 { self.node_to_delete = Some(idx); }
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
    /// アプリケーション終了時に設定を保存する
    fn drop(&mut self) {
        self.settings.save();
    }
}
