use eframe::egui;
use serde::{Deserialize, Serialize};
use crate::lang;
 
fn get_system_fonts() -> Vec<(String, String)> {
    let mut fonts = Vec::new();
    
    #[cfg(target_os = "windows")]
    {
        use winreg::enums::*;
        use winreg::RegKey;
        use winreg::types::FromRegValue;
        use winreg::HKEY;
        use std::path::Path;

        // システムフォルダとユーザーフォルダのパスを用意
        let system_fonts_dir = "C:\\Windows\\Fonts".to_string();
        // LOCALAPPDATA環境変数から動的にパスを取得する
        let user_fonts_dir = std::env::var("LOCALAPPDATA")
            .map(|p| format!("{}\\Microsoft\\Windows\\Fonts", p))
            .unwrap_or_else(|_| "".to_string());

        // レジストリから読み込んでリストに追加するクロージャ
        let mut load_fonts = |hkey: HKEY, subkey: &str, base_dir: &str| {
            let key = RegKey::predef(hkey);
            if let Ok(font_key) = key.open_subkey(subkey) {
                for item in font_key.enum_values() {
                    if let Ok((display_name, reg_value)) = item {
                        if let Ok(file_name) = String::from_reg_value(&reg_value) {
                            let clean_name = display_name
                                .replace(" (TrueType)", "")
                                .replace(" (OpenType)", "")
                                .replace(" & ", " / ");

                            let full_path = if Path::new(&file_name).is_absolute() {
                                file_name
                            } else {
                                format!("{}\\{}", base_dir, file_name)
                            };

                            if let Some(ext) = full_path.split('.').last() {
                                let ext = ext.to_lowercase();
                                if ext == "ttf" || ext == "ttc" || ext == "otf" {
                                    fonts.push((full_path, clean_name));
                                }
                            }
                        }
                    }
                }
            }
        };

        // 1. システム全体（全ユーザー）のフォントを読み込む
        load_fonts(
            HKEY_LOCAL_MACHINE, 
            "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Fonts", 
            &system_fonts_dir
        );

        // 2. 現在のユーザー専用のフォントを読み込む
        load_fonts(
            HKEY_CURRENT_USER, 
            "Software\\Microsoft\\Windows NT\\CurrentVersion\\Fonts", 
            &user_fonts_dir
        );
    }
    
    // アルファベット順にソート
    fonts.sort_by(|a: &(String, String), b: &(String, String)| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    // 同名フォントの重複を排除
    fonts.dedup_by(|a, b| a.1 == b.1);
    
    fonts
}

/// 設定ファイルのパス (Wasm環境では使用しないためコンパイルから除外)
#[cfg(not(target_arch = "wasm32"))]
fn settings_path() -> std::path::PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    exe_dir.join("renju_settings.json")
}

/// テキスト・コメントのエンコーディング（言語）
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub enum TextEncoding {
    ShiftJIS,   // 日本語 (Shift_JIS)
    UTF8,       // UTF-8 (多言語)
    GBK,        // 中国語簡体字 (GBK)
    Big5,       // 中国語繁体字 (Big5)
    EucKR,      // 韓国語 (EUC-KR)
}

impl TextEncoding {
    pub fn label(&self) -> &'static str {
        match self {
            TextEncoding::ShiftJIS => "日本語 (Shift_JIS)",
            TextEncoding::UTF8 => "UTF-8",
            TextEncoding::GBK => "中国語簡体字 (GBK)",
            TextEncoding::Big5 => "中国語繁体字 (Big5)",
            TextEncoding::EucKR => "韓国語 (EUC-KR)",
        }
    }

    pub const ALL: &'static [TextEncoding] = &[
        TextEncoding::ShiftJIS,
        TextEncoding::UTF8,
        TextEncoding::GBK,
        TextEncoding::Big5,
        TextEncoding::EucKR,
    ];

    /// encoding_rs のエンコーディングを返す
    pub fn to_encoding_rs(&self) -> &'static encoding_rs::Encoding {
        match self {
            TextEncoding::ShiftJIS => encoding_rs::SHIFT_JIS,
            TextEncoding::UTF8 => encoding_rs::UTF_8,
            TextEncoding::GBK => encoding_rs::GBK,
            TextEncoding::Big5 => encoding_rs::BIG5,
            TextEncoding::EucKR => encoding_rs::EUC_KR,
        }
    }
}

/// ディスクに保存される設定データ
fn default_true() -> bool { true }
fn default_top_branch_highlight_count() -> usize { 3 }

#[derive(Serialize, Deserialize)]
struct SettingsData {
    board_color: [f32; 3],
    text_encoding: TextEncoding,
    #[serde(default = "default_language")]
    ui_language: lang::Language,
    #[serde(default)]
    stone_shading: bool,
    #[serde(default)]
    show_numbers: bool,
    #[serde(default = "default_max_nodes")]
    pub max_nodes: usize,
    #[serde(default = "default_font_path")]
    pub font_path: String,
    #[serde(default = "default_wasm_button_size")]
    pub wasm_button_size: f32,
    #[serde(default = "default_true")]
    pub show_branches: bool,
    #[serde(default = "default_true")]
    pub show_branch_text: bool,
    #[serde(default = "default_true")]
    pub show_branch_count: bool,
    #[serde(default = "default_top_branch_highlight_count")]
    pub top_branch_highlight_count: usize,
}

fn default_max_nodes() -> usize {
    80_000_000
}

fn default_font_path() -> String {
    "C:\\Windows\\Fonts\\msgothic.ttc".to_string()
}

fn default_wasm_button_size() -> f32 {
    40.0
}

fn default_language() -> lang::Language {
    lang::Language::Japanese
}

/// アプリケーション設定
pub struct Settings {
    /// 連珠盤の背景色
    pub board_color: [f32; 3],
    /// Text及びコメントの言語（エンコーディング）
    pub text_encoding: TextEncoding,
    /// UI表示言語
    pub ui_language: lang::Language,
    /// 石に陰影をつけて描画するか
    pub stone_shading: bool,
    /// 石番号を表示するか
    pub show_numbers: bool,
    /// 最大読み込みノード数
    pub max_nodes: usize,
    /// フォントパス
    pub font_path: String,
    /// 設定ウィンドウの表示状態
    pub show_window: bool,
    #[allow(dead_code)]
    pub available_fonts: Vec<(String, String)>, 
    pub wasm_button_size: f32,
    pub show_branches: bool,
    pub show_branch_text: bool,
    pub show_branch_count: bool,
    pub top_branch_highlight_count: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            board_color: [250.0 / 255.0, 230.0 / 255.0, 166.0 / 255.0],
            text_encoding: TextEncoding::ShiftJIS,
            ui_language: lang::Language::Japanese,
            stone_shading: true,
            show_numbers: true,
            max_nodes: 80_000_000,
            font_path: "C:\\Windows\\Fonts\\msgothic.ttc".to_string(),
            show_window: false,
            available_fonts: get_system_fonts(),
            wasm_button_size: 40.0,
            show_branches: true,
            show_branch_text: true,
            show_branch_count: true,
            top_branch_highlight_count: 3,
        }
    }
}

impl Settings {
    /// ファイルから設定を読み込む。失敗時はデフォルト値を返す。
    pub fn load() -> Self {
        let fonts = get_system_fonts();
        
        // --- ネイティブ（Windows等）の読み込み ---
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = settings_path();
            if let Ok(json) = std::fs::read_to_string(&path) {
                if let Ok(data) = serde_json::from_str::<SettingsData>(&json) {
                    return Self::from_data(data, fonts);
                }
            }
        }

        // --- Wasm（ブラウザ）の読み込み ---
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
                if let Ok(Some(json)) = storage.get_item("renju_settings") {
                    if let Ok(data) = serde_json::from_str::<SettingsData>(&json) {
                        return Self::from_data(data, fonts);
                    }
                }
            }
        }
        
        Self::default()
    }

    pub fn save(&self) {
        let data = SettingsData {
            board_color: self.board_color,
            text_encoding: self.text_encoding,
            ui_language: self.ui_language,
            stone_shading: self.stone_shading,
            show_numbers: self.show_numbers,
            max_nodes: self.max_nodes,
            font_path: self.font_path.clone(),
            wasm_button_size: self.wasm_button_size,
            show_branches: self.show_branches,
            show_branch_text: self.show_branch_text,
            show_branch_count: self.show_branch_count,
            top_branch_highlight_count: self.top_branch_highlight_count,
        };
        
        if let Ok(json) = serde_json::to_string_pretty(&data) {
            // --- ネイティブの保存 ---
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = std::fs::write(settings_path(), json);
            }

            // --- Wasmの保存 ---
            #[cfg(target_arch = "wasm32")]
            {
                if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
                    let _ = storage.set_item("renju_settings", &json);
                }
            }
        }
    }

    // 補助関数：SettingsDataからSettingsを作成する
    fn from_data(data: SettingsData, fonts: Vec<(String, String)>) -> Self {
        Self {
            board_color: data.board_color,
            text_encoding: data.text_encoding,
            ui_language: data.ui_language,
            stone_shading: data.stone_shading,
            show_numbers: data.show_numbers,
            max_nodes: data.max_nodes,
            font_path: data.font_path,
            show_window: false,
            available_fonts: fonts,
            wasm_button_size: data.wasm_button_size,
            show_branches: data.show_branches,
            show_branch_text: data.show_branch_text,
            show_branch_count: data.show_branch_count,
            top_branch_highlight_count: data.top_branch_highlight_count,
        }
    }

    /// 現在の翻訳テーブルを返す
    pub fn tr(&self) -> &'static lang::Tr {
        lang::get(self.ui_language)
    }

    /// 盤面の背景色を egui::Color32 として返す
    pub fn board_color32(&self) -> egui::Color32 {
        egui::Color32::from_rgb(
            (self.board_color[0] * 255.0) as u8,
            (self.board_color[1] * 255.0) as u8,
            (self.board_color[2] * 255.0) as u8,
        )
    }

    /// 設定ウィンドウを描画する。保存が押されたらtrueを返す。
    pub fn show_settings_window(&mut self, ctx: &egui::Context) -> bool {
        let tr = lang::get(self.ui_language);
        let mut open = self.show_window;
        let mut saved = false;
        egui::Window::new(tr.settings_title)
            .id(egui::Id::new("settings_window_fixed_id"))
            .open(&mut open)
            .resizable(false)
            .default_width(300.0)
            .show(ctx, |ui| {
                ui.heading(tr.board_section);
                ui.horizontal(|ui| {
                    ui.label(tr.board_color);
                    ui.color_edit_button_rgb(&mut self.board_color);
                });
                ui.checkbox(&mut self.stone_shading, tr.stone_shading);
                ui.checkbox(&mut self.show_numbers, tr.show_numbers);

                ui.add_space(4.0);
                ui.checkbox(&mut self.show_branches, tr.show_branches);
                if self.show_branches {
                    ui.horizontal(|ui| {
                        ui.add_space(20.0);
                        ui.checkbox(&mut self.show_branch_text, tr.show_branch_text);
                        ui.checkbox(&mut self.show_branch_count, tr.show_branch_count);
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(20.0);
                        ui.label(tr.top_branch_highlight_count);
                        ui.add(egui::DragValue::new(&mut self.top_branch_highlight_count).range(0..=15));
                    });
                }

               #[cfg(not(target_arch = "wasm32"))]
                {
                    ui.add_space(8.0);
                    ui.separator();
                    ui.heading(tr.font_section);
                    ui.horizontal(|ui| {
                        ui.label(tr.font_label);
                        
                        let current_display = self.available_fonts.iter()
                            .find(|(path, _)| path == &self.font_path)
                            .map(|(_, name)| name.clone())
                            .unwrap_or_else(|| {
                                std::path::Path::new(&self.font_path)
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or(&self.font_path)
                                    .to_string()
                            });

                        egui::ComboBox::from_id_salt("font_combo")
                            .width(200.0) 
                            .selected_text(&current_display)
                            .show_ui(ui, |ui| {
                                for (path, name) in &self.available_fonts {
                                    ui.selectable_value(&mut self.font_path, path.clone(), name);
                                }
                            });
                    });
                }

                ui.add_space(8.0);
                ui.separator();
                ui.heading(tr.encoding_section);
                ui.horizontal(|ui| {
                    ui.label(tr.max_nodes_label);
                    ui.add(egui::DragValue::new(&mut self.max_nodes).speed(10000).range(100000..=100_000_000));
                });
                ui.add_space(4.0);
                ui.label(tr.encoding_label);
                egui::ComboBox::from_id_salt("encoding_combo")
                    .selected_text(self.text_encoding.label())
                    .show_ui(ui, |ui| {
                        for &enc in TextEncoding::ALL {
                            ui.selectable_value(&mut self.text_encoding, enc, enc.label());
                        }
                    });

                ui.add_space(8.0);
                ui.separator();
                ui.heading(tr.language_section);
                ui.horizontal(|ui| {
                    ui.label(tr.language_label);
                    egui::ComboBox::from_id_salt("language_combo")
                        .selected_text(self.ui_language.label())
                        .show_ui(ui, |ui| {
                            for &l in lang::Language::ALL {
                                ui.selectable_value(&mut self.ui_language, l, l.label());
                            }
                        });
                });
 
                #[cfg(target_arch = "wasm32")]
                {
                    ui.add_space(8.0);
                    ui.separator();
                    ui.heading("UI Layout (Wasm)");
                    ui.horizontal(|ui| {
                        ui.label("Toolbar Button Size:");
                        ui.add(egui::Slider::new(&mut self.wasm_button_size, 20.0..=80.0));
                    });
                }

                ui.add_space(12.0);
                if ui.button(tr.save).clicked() {
                    self.save();
                    saved = true;
                }
            });
        self.show_window = open;
        saved
    }
}