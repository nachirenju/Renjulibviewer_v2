// 24バイトに圧縮された局面ノードおよびテキストプールの定義を行う
use rustc_hash::{FxHashMap, FxHasher};
use std::hash::{Hash, Hasher};

pub const NO_NODE: u32 = u32::MAX;
pub const NO_TEXT: u32 = u32::MAX;

pub const MASK_TEXT: u8 = 0x01;
pub const MASK_COMMENT: u8 = 0x08;
pub const MASK_SIBLING: u8 = 0x80;
pub const MASK_NOCHILD: u8 = 0x40;

/// アリーナアロケータベースのTextPool
#[derive(Clone)]
pub struct TextPool {
    // 1. 全てのテキストを一直線に保存する巨大な単一バッファ
    pub buffer: Vec<u8>,
    // 2. 各テキストの (オフセット, 長さ) を記録
    pub spans: Vec<(u32, u32)>,
    // 3. テキストのハッシュ値(u64)からIDを引くためのルックアップ
    pub lookup: FxHashMap<u64, u32>,
    // 4. ハッシュ衝突時のチェイン用（ID -> 次のID）
    pub next_chain: Vec<u32>,
}

impl Default for TextPool {
    fn default() -> Self {
        Self {
            // メモリの再確保を防ぐため、初期容量を大きめに確保しておく
            buffer: Vec::with_capacity(1024 * 1024), // 1MB
            spans: Vec::with_capacity(10000),
            lookup: FxHashMap::default(),
            next_chain: Vec::with_capacity(10000),
        }
    }
}

impl TextPool {
    pub fn new() -> Self {
        Self::default()
    }

    /// スライスからハッシュ値を直接計算する（キー用の `Vec<u8>` 生成を回避）
    #[inline(always)]
    fn hash_bytes(bytes: &[u8]) -> u64 {
        let mut hasher = FxHasher::default();
        bytes.hash(&mut hasher);
        hasher.finish()
    }

    pub fn get_or_insert(&mut self, text: &[u8]) -> u32 {
        let hash = Self::hash_bytes(text);

        // 同じハッシュ値を持つテキストが既に存在するかチェック
        if let Some(&first_id) = self.lookup.get(&hash) {
            let mut current_id = first_id;
            loop {
                let (offset, len) = self.spans[current_id as usize];
                let existing = &self.buffer[offset as usize .. (offset + len) as usize];
                if existing == text {
                    return current_id; // 完全一致 → 既存IDを返す
                }
                // ハッシュ衝突時のチェインを辿る
                let next = self.next_chain[current_id as usize];
                if next == NO_TEXT {
                    break;
                }
                current_id = next;
            }
        }

        // 見つからなかった場合はアリーナに新規追加
        let new_id = self.spans.len() as u32;
        let offset = self.buffer.len() as u32;
        let len = text.len() as u32;

        self.buffer.extend_from_slice(text);
        self.spans.push((offset, len));
        
        // 衝突時チェインの更新（先頭に追加していくスタイル）
        if let Some(&existing_first_id) = self.lookup.get(&hash) {
            self.next_chain.push(existing_first_id);
        } else {
            self.next_chain.push(NO_TEXT);
        }
        self.lookup.insert(hash, new_id);

        new_id
    }

    pub fn get(&self, id: u32) -> Option<&[u8]> {
        if id == NO_TEXT || (id as usize) >= self.spans.len() {
            None
        } else {
            let (offset, len) = self.spans[id as usize];
            Some(&self.buffer[offset as usize .. (offset + len) as usize])
        }
    }
}

/// 24バイトに極限圧縮された局面ノード (Hot Data分離版)
#[derive(Debug, Clone)]
pub struct RenLibNode {
    pub hash: u64,          // 8 bytes
    pub parent: u32,        // 4 bytes
    pub child: u32,         // 4 bytes
    pub sibling: u32,       // 4 bytes
    // x (8 bit) + y (8 bit) + depth (8 bit) + 予約領域 (8 bit)
    pub move_info: u32,     // 4 bytes
} // Total: 24 bytes!

impl RenLibNode {
    #[inline(always)]
    pub fn new(x: i8, y: i8, parent: u32, child: u32, sibling: u32, hash: u64, depth: u8) -> Self {
        let ux = x as u8 as u32;
        let uy = y as u8 as u32;
        let ud = depth as u32;
        let move_info = ux | (uy << 8) | (ud << 16);
        
        Self { hash, parent, child, sibling, move_info }
    }

    #[inline(always)] pub fn x(&self) -> i8 { (self.move_info & 0xFF) as i8 }
    #[inline(always)] pub fn y(&self) -> i8 { ((self.move_info >> 8) & 0xFF) as i8 }
    #[inline(always)] pub fn depth(&self) -> u8 { ((self.move_info >> 16) & 0xFF) as u8 }
    
    #[inline(always)]
    pub fn set_x(&mut self, x: i8) {
        self.move_info = (self.move_info & 0xFFFFFF00) | (x as u8 as u32);
    }
    
    #[inline(always)]
    pub fn set_y(&mut self, y: i8) {
        self.move_info = (self.move_info & 0xFFFF00FF) | ((y as u8 as u32) << 8);
    }
    
    #[inline(always)]
    pub fn set_depth(&mut self, depth: u8) {
        self.move_info = (self.move_info & 0xFF00FFFF) | ((depth as u32) << 16);
    }

}
