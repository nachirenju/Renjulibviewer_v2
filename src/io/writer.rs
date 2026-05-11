// 局面ツリーやデータベースを .lib および .db 形式でファイルに保存する
use byteorder::{LittleEndian, WriteBytesExt};

use crate::core::node::*;
use crate::core::zobrist::Zobrist;

/// 解析済みのノードを `.lib` や `.db` 形式でバイナリバッファに書き出すライター
#[allow(dead_code)]
pub struct RenLibWriter {
    pub buffer: Vec<u8>,
    pub encoding: &'static encoding_rs::Encoding,
}

#[allow(dead_code)]
impl RenLibWriter {
    pub fn new(encoding: &'static encoding_rs::Encoding) -> Self {
        Self {
            buffer: Vec::with_capacity(1024 * 1024),
            encoding,
        }
    }

    pub fn write_string(&mut self, s: &str) {
        let (encoded, _, _) = self.encoding.encode(s);
        self.buffer.extend_from_slice(&encoded);
        self.buffer.push(0); 
        // 偶数バイト境界に合わせるためのパディングを追加する
        if (encoded.len() + 1) % 2 != 0 {
            self.buffer.push(0); 
        }
    }

    pub fn save(&mut self, nodes: &[RenLibNode], node_texts: &[u32], pool: &TextPool) -> Vec<u8> {
        self.buffer.clear();
        self.buffer.extend_from_slice(&[0u8; 20]);

        if nodes.is_empty() { return self.buffer.clone(); }

        fn next_valid(nodes: &[RenLibNode], start: u32) -> u32 {
            let mut cur = start;
            // 削除フラグ(-2)のノードをスキップして、次に有効なノードを探す
            while let Some(node) = nodes.get(cur as usize) {
                if node.x() != -2 { return cur; }
                cur = node.sibling;
            }
            NO_NODE
        }

        let mut stack: Vec<u32> = vec![0];

        // スタックを用いた深さ優先探索（DFS）でノードをバイナリバッファに順次書き出す
        while let Some(idx) = stack.pop() {
            let node = &nodes[idx as usize];

            let real_child   = next_valid(nodes, node.child);
            let real_sibling = next_valid(nodes, node.sibling);

            let move_byte = if node.x() >= 0 && node.y() >= 0 {
                ((node.x() + 1) as u8) | ((node.y() as u8) << 4)
            } else {
                0u8
            };
            self.buffer.push(move_byte);

            let mut flags = 0u8;
            if real_child == NO_NODE  { flags |= MASK_NOCHILD; }
            if idx != 0 && real_sibling != NO_NODE { flags |= MASK_SIBLING; }
            
            let texts = node_texts.get(idx as usize).copied().unwrap_or(0xFFFF | (0xFFFF << 16));
            let mut comment_id = texts & 0xFFFF;
            if comment_id == 0xFFFF { comment_id = NO_TEXT; }
            let mut text_id = (texts >> 16) & 0xFFFF;
            if text_id == 0xFFFF { text_id = NO_TEXT; }

            if comment_id != NO_TEXT { flags |= MASK_COMMENT; }
            if text_id != NO_TEXT    { flags |= MASK_TEXT; }
            self.buffer.push(flags);

            if (flags & MASK_TEXT) != 0 {
                self.buffer.push(0);
                self.buffer.push(0);
            }

            // コメントが存在する場合、バッファにコメントデータを書き込む
            if comment_id != NO_TEXT {
                if let Some(comment_bytes) = pool.get(comment_id) {
                    self.buffer.extend_from_slice(comment_bytes);
                    self.buffer.push(0);
                    // 偶数バイト境界に合わせるためのパディングを追加する
                    if (comment_bytes.len() + 1) % 2 != 0 { self.buffer.push(0); }
                }
            }
            
            // テキストが存在する場合、バッファにテキストデータを書き込む
            if text_id != NO_TEXT {
                if let Some(text_bytes) = pool.get(text_id) {
                    self.buffer.extend_from_slice(text_bytes);
                    self.buffer.push(0);
                    // 偶数バイト境界に合わせるためのパディングを追加する
                    if (text_bytes.len() + 1) % 2 != 0 { self.buffer.push(0); }
                }
            }

            if idx != 0 {
                if real_sibling != NO_NODE { stack.push(real_sibling); }
            }
            if real_child != NO_NODE { stack.push(real_child); }
        }

        self.buffer.clone()
    }

    pub fn save_db(&mut self, nodes: &[RenLibNode], node_texts: &[u32], pool: &TextPool) -> Vec<u8> {
        self.buffer.clear();

        if nodes.is_empty() {
            self.buffer.extend_from_slice(&0u32.to_le_bytes());
            return self.buffer.clone();
        }

        struct DbEntry {
            black_stones: Vec<(i8, i8)>,
            white_stones: Vec<(i8, i8)>,
            label: u8,
            value: i16,
            comment: String,
            depth: u8,
            btxt_list: Vec<(i8, i8, String)>,
        }

        let mut entries: Vec<DbEntry> = Vec::new();
        let mut node_to_entry: rustc_hash::FxHashMap<usize, usize> = rustc_hash::FxHashMap::default();
        let mut btxt_map: rustc_hash::FxHashMap<usize, Vec<(i8, i8, String)>> = rustc_hash::FxHashMap::default();

        let mut move_stack: Vec<(i8, i8)> = Vec::new();
        let mut dfs_stack: Vec<(usize, bool)> = Vec::new(); 
        dfs_stack.push((0, true));

        // DFSでツリーをトラバースし、各局面の盤面状態とテキスト/コメントを収集する
        while let Some((idx, entering)) = dfs_stack.pop() {
            if idx >= nodes.len() { continue; }
            let node = &nodes[idx];
            if node.x() == -2 { continue; }

            if entering {
                if idx != 0 && node.x() >= 0 && node.y() >= 0 {
                    move_stack.push((node.x(), node.y()));
                }

                let mut black = Vec::new();
                let mut white = Vec::new();
                // 手順の履歴から黒石と白石のリストを分離する
                for (i, &(mx, my)) in move_stack.iter().enumerate() {
                    if i % 2 == 0 { black.push((mx, my)); } else { white.push((mx, my)); }
                }

                let depth = move_stack.len() as u8;
                
                let mut node_text_str = None;
                let mut node_comment_str = None;
                if let Some(&texts) = node_texts.get(idx) {
                    let tid = (texts >> 16) & 0xFFFF;
                    if tid != 0xFFFF {
                        if let Some(b) = pool.get(tid) {
                            node_text_str = Some(self.encoding.decode(b).0.into_owned());
                        }
                    }
                    let cid = texts & 0xFFFF;
                    if cid != 0xFFFF {
                        if let Some(b) = pool.get(cid) {
                            node_comment_str = Some(self.encoding.decode(b).0.into_owned());
                        }
                    }
                }

                let (label, value) = if let Some(t) = node_text_str {
                    if self.is_label_value_text(&t) { self.text_to_label_value(&t, depth as usize) } else { (0, 0) }
                } else { (0, 0) };

                let comment = node_comment_str.unwrap_or_default();

                let entry_idx = entries.len();
                node_to_entry.insert(idx, entry_idx);
                entries.push(DbEntry { black_stones: black, white_stones: white, label, value, comment, depth, btxt_list: Vec::new() });

                dfs_stack.push((idx, false));

                let mut children = Vec::new();
                let mut c = node.child;
                // 子ノードを走査し、有効なノードをリストアップする
                while c != NO_NODE {
                    if nodes[c as usize].x() != -2 { children.push(c as usize); }
                    c = nodes[c as usize].sibling;
                }
                
                // 子ノードをスタックに逆順で積み、正しい順序でDFSを進める
                for &c_idx in children.iter().rev() {
                    dfs_stack.push((c_idx, true));
                }

                if let Some(&entry_idx) = node_to_entry.get(&idx) {
                    // 各子ノードのテキスト情報を収集し、盤面のテキストマーク（BTXT）として保存する
                    for &c_idx in &children {
                        if let Some(&texts) = node_texts.get(c_idx) {
                            let tid = (texts >> 16) & 0xFFFF;
                            if tid != 0xFFFF {
                                if let Some(b) = pool.get(tid) {
                                    let text = self.encoding.decode(b).0.into_owned();
                                    if !self.is_label_value_text(&text) {
                                        if nodes[c_idx].x() >= 0 && nodes[c_idx].y() >= 0 {
                                            btxt_map.entry(entry_idx).or_default()
                                                .push((nodes[c_idx].x(), nodes[c_idx].y(), text));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                if idx != 0 && node.x() >= 0 && node.y() >= 0 {
                    move_stack.pop();
                }
            }
        }

        let mut final_entries: rustc_hash::FxHashMap<Vec<i8>, DbEntry> = rustc_hash::FxHashMap::default();

        // 収集したエントリについて、対称性を考慮して正規化された盤面状態（一意なキー）を計算・統合する
        for (entry_idx, entry) in entries.into_iter().enumerate() {
            let mut best_b = entry.black_stones.clone();
            let mut best_w = entry.white_stones.clone();
            best_b.sort_unstable_by_key(|&(x, y)| (x as i32) * 15 + (y as i32));
            best_w.sort_unstable_by_key(|&(x, y)| (x as i32) * 15 + (y as i32));
            let mut best_t = 0;

            // 8つの対称変換（回転・反転）をすべて試し、辞書順で最小となる盤面表現を探す
            for t in 1..8 {
                let mut tb: Vec<(i8, i8)> = entry.black_stones.iter().map(|&(x, y)| {
                    let id = Zobrist::get_transformed_idx(x, y, t);
                    ((id % 15) as i8, (id / 15) as i8)
                }).collect();
                tb.sort_unstable_by_key(|&(x, y)| (x as i32) * 15 + (y as i32));

                let mut tw: Vec<(i8, i8)> = entry.white_stones.iter().map(|&(x, y)| {
                    let id = Zobrist::get_transformed_idx(x, y, t);
                    ((id % 15) as i8, (id / 15) as i8)
                }).collect();
                tw.sort_unstable_by_key(|&(x, y)| (x as i32) * 15 + (y as i32));

                let mut is_less = false;
                let mut is_eq = true;
                
                // 黒石のリストを比較し、辞書順で小さいかを判定する
                for i in 0..tb.len().min(best_b.len()) {
                    let k1 = (tb[i].0 as i32) * 15 + tb[i].1 as i32;
                    let k2 = (best_b[i].0 as i32) * 15 + best_b[i].1 as i32;
                    if k1 < k2 { is_less = true; is_eq = false; break; }
                    if k1 > k2 { is_eq = false; break; }
                }
                
                if is_eq && tb.len() != best_b.len() {
                    is_less = tb.len() < best_b.len();
                    is_eq = false;
                }
                
                if is_eq {
                    // 白石のリストを比較し、辞書順で小さいかを判定する
                    for i in 0..tw.len().min(best_w.len()) {
                        let k1 = (tw[i].0 as i32) * 15 + tw[i].1 as i32;
                        let k2 = (best_w[i].0 as i32) * 15 + best_w[i].1 as i32;
                        if k1 < k2 { is_less = true; is_eq = false; break; }
                        if k1 > k2 { is_eq = false; break; }
                    }
                    if is_eq && tw.len() != best_w.len() {
                        is_less = tw.len() < best_w.len();
                    }
                }

                if is_less { best_b = tb; best_w = tw; best_t = t; }
            }

            let mut map_key = Vec::new();
            // 正規化された黒石の座標をマップキーに追加する
            for &(x, y) in &best_b { map_key.push(x); map_key.push(y); }
            map_key.push(-1); 
            // 正規化された白石の座標をマップキーに追加する
            for &(x, y) in &best_w { map_key.push(x); map_key.push(y); }

            let mut c_btxt = Vec::new();
            if let Some(btxts) = btxt_map.get(&entry_idx) {
                // 盤面のテキストマークも採用された対称変換に合わせて座標を変換する
                for (bx, by, txt) in btxts {
                    let id = Zobrist::get_transformed_idx(*bx, *by, best_t);
                    c_btxt.push(((id % 15) as i8, (id / 15) as i8, txt.clone()));
                }
            }

            let e = final_entries.entry(map_key).or_insert(DbEntry {
                black_stones: best_b, white_stones: best_w, label: 0, value: 0, comment: String::new(), depth: entry.depth, btxt_list: Vec::new(),
            });

            if entry.label != 0 || entry.value != 0 {
                e.label = entry.label; e.value = entry.value;
            }
            if !entry.comment.is_empty() {
                if !e.comment.is_empty() { e.comment.push('\n'); }
                e.comment.push_str(&entry.comment);
            }
            // 変換済みのテキストマークを最終的なエントリに追加・更新する
            for (bx, by, txt) in c_btxt {
                if let Some(existing) = e.btxt_list.iter_mut().find(|(ex, ey, _)| *ex == bx && *ey == by) {
                    existing.2 = txt;
                } else {
                    e.btxt_list.push((bx, by, txt));
                }
            }
        }

        let num_records = (final_entries.len() + 1) as u32;
        self.buffer.extend_from_slice(&num_records.to_le_bytes());

        // メタデータヘッダーブロックの書き込み
        {
            let metadata = b"charset=\"UTF-8\"";
            let num_key_bytes: u16 = 3;
            self.buffer.extend_from_slice(&num_key_bytes.to_le_bytes());
            self.buffer.extend_from_slice(&[2u8, 0, 0]); 

            let num_record_bytes: u16 = 5 + metadata.len() as u16;
            self.buffer.extend_from_slice(&num_record_bytes.to_le_bytes());
            self.buffer.extend_from_slice(&[0u8; 5]); 
            self.buffer.extend_from_slice(metadata);
        }

        // 正規化してまとめられた各局面データをDB形式でバイナリバッファに書き出す
        for (_, entry) in final_entries {
            let num_stones = entry.black_stones.len() + entry.white_stones.len();
            let num_key_bytes: u16 = 3 + (num_stones as u16) * 2;
            
            self.buffer.extend_from_slice(&num_key_bytes.to_le_bytes());
            self.buffer.push(2); 
            self.buffer.push(15); 
            self.buffer.push(15); 
            
            // 黒石の座標データをバイナリに書き込む
            for &(x, y) in &entry.black_stones {
                self.buffer.push(x as u8); self.buffer.push(y as u8);
            }
            // 白石の座標データをバイナリに書き込む
            for &(x, y) in &entry.white_stones {
                self.buffer.push(x as u8); self.buffer.push(y as u8);
            }

            let mut text_field = Vec::new();

            fn coord_to_hex(c: i8) -> u8 {
                let c = c.max(0).min(14) as u8;
                if c < 10 { b'0' + c } else { b'A' + (c - 10) }
            }
            
            if !entry.btxt_list.is_empty() {
                text_field.extend_from_slice(b"@BTXT@");
                // 盤面のテキストマーク（BTXT）を規定のフォーマットで文字列にエンコードする
                for (bx, by, txt) in &entry.btxt_list {
                    text_field.push(coord_to_hex(*bx)); text_field.push(coord_to_hex(*by));
                    text_field.extend_from_slice(txt.as_bytes()); text_field.push(b'\n');
                }
                if !entry.comment.is_empty() {
                    text_field.push(0x08); text_field.extend_from_slice(entry.comment.as_bytes());
                }
            } else if !entry.comment.is_empty() {
                text_field.extend_from_slice(entry.comment.as_bytes());
            }

            let has_data = entry.label != 0 || entry.value != 0 || !text_field.is_empty();
            if has_data {
                let num_record_bytes: u16 = 5 + text_field.len() as u16;
                self.buffer.extend_from_slice(&num_record_bytes.to_le_bytes());
                self.buffer.push(entry.label);
                self.buffer.write_i16::<LittleEndian>(entry.value).unwrap();
                let depthbound: u16 = if entry.value != 0 { 3 } else { 0 };
                self.buffer.write_u16::<LittleEndian>(depthbound).unwrap();
                self.buffer.extend_from_slice(&text_field);
            } else {
                let num_record_bytes: u16 = 0;
                self.buffer.extend_from_slice(&num_record_bytes.to_le_bytes());
            }
        }

        self.buffer.clone()
    }

    fn text_to_label_value(&self, text: &str, depth: usize) -> (u8, i16) {
        if text == "W" { return (b'W', 0); }
        if text == "L" { return (b'L', 0); }
        
        if text.starts_with('W') {
            if let Ok(n) = text[1..].parse::<i32>() {
                let steps = n as i32 - depth as i32;
                if steps > 0 { return (b'W', -(30000 - steps as i16 + 1)); }
                return (b'W', 0);
            }
        }
        
        if text.starts_with('L') {
            if let Ok(n) = text[1..].parse::<i32>() {
                let steps = n as i32 - depth as i32;
                if steps > 0 { return (b'L', 30000 - steps as i16 + 1); }
                return (b'L', 0);
            }
        }
        
        if let Ok(pct) = text.trim_end_matches('%').parse::<i32>() {
            if pct > 0 && pct < 100 {
                let wr = pct as f64 / 100.0;
                let v = (250.0 * (wr / (1.0 - wr)).ln()) as i16;
                return (0, v.clamp(-29500, 29500));
            }
        }
        (0, 0) 
    }

    fn is_label_value_text(&self, text: &str) -> bool {
        if text == "W" || text == "L" { return true; }
        if text.starts_with('W') && text[1..].parse::<i32>().is_ok() { return true; }
        if text.starts_with('L') && text[1..].parse::<i32>().is_ok() { return true; }
        if text.trim_end_matches('%').parse::<i32>().is_ok() { return true; }
        false
    }
}