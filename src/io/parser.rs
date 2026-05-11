use std::io::{self, Read, BufRead};
use rustc_hash::FxHashMap;
use encoding_rs::SHIFT_JIS;

#[cfg(not(target_arch = "wasm32"))]
use rayon::prelude::*;

use crate::core::node::*;
use crate::core::zobrist::Zobrist;
use crate::core::hash_board::Board;

// バイト列のパース状態を管理する構造体を定義する
struct ParserContext<R: BufRead> {
    reader: R,
    buffer: Vec<u8>,
}

// ParserContextに対するメソッド群を実装する
impl<R: BufRead> ParserContext<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buffer: Vec::with_capacity(1024),
        }
    }

    // リトルエンディアンで2バイトの数値を読み込む
    #[inline(always)]
    fn read_u16_le(&mut self) -> io::Result<u16> {
        let mut buf = [0u8; 2];
        self.reader.read_exact(&mut buf)?;
        Ok(u16::from_le_bytes(buf))
    }

    // リトルエンディアンで4バイトの数値を読み込む
    #[inline(always)]
    fn read_u32_le(&mut self) -> io::Result<u32> {
        let mut buf = [0u8; 4];
        self.reader.read_exact(&mut buf)?;
        Ok(u32::from_le_bytes(buf))
    }

    // 指定されたバイト数だけ読み込み位置をスキップする
    #[inline(always)]
    fn skip(&mut self, mut n: usize) -> io::Result<()> {
        while n > 0 {
            let buf = self.reader.fill_buf()?;
            if buf.is_empty() {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "EOF"));
            }
            let consumed = std::cmp::min(n, buf.len());
            self.reader.consume(consumed);
            n -= consumed;
        }
        Ok(())
    }

    // 指定された長さのバイトスライスを取得する
    #[inline(always)]
    fn read_slice(&mut self, len: usize) -> io::Result<&[u8]> {
        self.buffer.resize(len, 0);
        self.reader.read_exact(&mut self.buffer)?;
        Ok(&self.buffer)
    }

}

// 棋譜ファイルのパースおよび木構造構築を行う構造体を定義する
pub struct RenLibParser {
    pub nodes: Vec<RenLibNode>,
    pub hash_table: FxHashMap<u64, u32>, 
    pub node_texts: Vec<u32>, // FxHashMap から Vec に
    pub max_nodes: usize,
    pub encoding: &'static encoding_rs::Encoding,
    pub text_pool: TextPool, 
}

// 深さ優先探索時の状態を保持するスタックフレーム構造体を定義する
struct StackFrame {
    node_idx: usize,
    stage: u8,
    has_child: bool,
    has_sibling: bool,
    made_move: bool,
}

// パース中の一時的なノード情報を保持する構造体を定義する
struct NodeInfo {
    idx: usize,
    has_child: bool,
    has_sibling: bool,
}

// 並列処理での計算結果を格納する構造体
struct ParentSearchResult {
    parent_idx: u32,
    inv_t: u8,
    rx: i8,
    ry: i8,
}

// RenLibParserに対するメソッド群を実装する
impl RenLibParser {
    // RenLibParserの初期化を行う
    pub fn new(max_nodes: usize) -> Self {
        Self {
            nodes: Vec::with_capacity(20000),
            hash_table: FxHashMap::default(), 
            node_texts: Vec::new(), // default から Vec::new() に
            max_nodes,
            encoding: SHIFT_JIS,
            text_pool: TextPool::new(),
        }
    }

    // 文字エンコーディングを設定する
    pub fn set_encoding(&mut self, encoding: &'static encoding_rs::Encoding) {
        self.encoding = encoding;
    }



    // .lib ファイル専用：スライスから直接ゼロコピーでノードを読み込む
    #[inline(always)]
    fn read_node_from_slice(&mut self, data: &[u8], offset: &mut usize) -> Option<NodeInfo> {
        if self.nodes.len() >= self.max_nodes || *offset + 1 >= data.len() {
            return None;
        }

        let move_byte = data[*offset];
        let flag = data[*offset + 1];
        *offset += 2;

        let (x, y) = if move_byte == 0 {
            (-1, -1) 
        } else {
            (( (move_byte & 0x0f) as i8 - 1), ( (move_byte >> 4) as i8))
        };

        if (flag & MASK_TEXT) != 0 { 
            *offset += 2; // テキストフラグ用パディングスキップ
        }

        // ゼロコピーで文字列（ヌル終端）を抽出するヘルパー関数
        let mut extract_string = || -> u32 {
            let start = *offset;
            // イテレータを使って一気にヌル文字を探す（SIMD最適化）
            if let Some(null_pos) = data[*offset..].iter().position(|&b| b == 0) {
                *offset += null_pos;
            } else {
                *offset = data.len();
            }
            let str_len = *offset - start;
            // ゼロコピーでスライスを切り出す
            let slice = &data[start..*offset];
            
            if *offset < data.len() { *offset += 1; } // ヌル文字をスキップ
            // 2バイト境界に合わせるためのパディング
            if (str_len + 1) % 2 != 0 {
                if *offset < data.len() { *offset += 1; }
            }
            
            if slice.is_empty() { NO_TEXT } else { self.text_pool.get_or_insert(slice) }
        };

        let comment_id = if (flag & MASK_COMMENT) != 0 { extract_string() } else { NO_TEXT };
        let text_id = if (flag & MASK_TEXT) != 0 { extract_string() } else { NO_TEXT };

        let idx = self.nodes.len();
        self.nodes.push(RenLibNode::new(x, y, NO_NODE, NO_NODE, NO_NODE, 0, 0));

        let texts = if comment_id != NO_TEXT || text_id != NO_TEXT {
            let cid = std::cmp::min(comment_id, 0xFFFF);
            let tid = std::cmp::min(text_id, 0xFFFF);
            cid | (tid << 16)
        } else {
            0xFFFF | (0xFFFF << 16)
        };
        self.node_texts.push(texts);

        Some(NodeInfo {
            idx,
            has_child: (flag & MASK_NOCHILD) == 0,
            has_sibling: (flag & MASK_SIBLING) != 0,
        })
    }

    // .libファイルの全データから木構造を構築する
    pub fn parse_all(&mut self, data: &[u8], board: &mut Board) -> io::Result<()> {
        if data.len() < 20 { return Ok(()); } // ヘッダーサイズ未満は弾く
        let mut offset = 20; // ヘッダー(20バイト)をスキップして開始
        
        // 1ノード平均3バイトで見積もり
        let estimated_nodes = (data.len() / 3).max(1000).min(self.max_nodes);
        
        self.nodes.clear();
        self.node_texts.clear();
        self.hash_table.clear(); // ここでの hash_table.reserve は削除

        self.nodes.reserve(estimated_nodes);
        self.node_texts.reserve(estimated_nodes);

        // ハッシュを一時的に溜め込むフラットな配列
        let mut flat_hashes = Vec::with_capacity(estimated_nodes);

        self.nodes.push(RenLibNode::new(-1, -1, NO_NODE, NO_NODE, NO_NODE, 0, 0));
        self.node_texts.push(0xFFFF | (0xFFFF << 16)); // ルートノード用のテキスト初期値を追加

        // ルートノードの読み込みを試行する
        let mut root_info = match self.read_node_from_slice(data, &mut offset) {
            Some(info) => info, None => return Ok(()),
        };

        // ルートノードがパス指定（座標が負）の場合の移譲処理を行う
        if self.nodes[root_info.idx].x() < 0 {
            // コメントやテキストが存在すれば仮想ルートノードに引き継ぐ
            let texts = self.node_texts[root_info.idx];
            if texts != 0xFFFF_FFFF {
                self.node_texts[0] = texts;
            }
            self.node_texts.pop(); // 古いルートのテキスト情報を削除
            let has_child = root_info.has_child;
            self.nodes.pop(); 
            // 子ノードが存在すればそれを新しい実質ルートとしてパースする
            if has_child {
                // 新ルートの読み込みを試行する
                root_info = match self.read_node_from_slice(data, &mut offset) {
                    Some(info) => info, None => return Ok(()),
                };
            } else {
                return Ok(()); 
            }
        }

        self.nodes[0].child = root_info.idx as u32;
        self.nodes[root_info.idx].parent = 0;

        let mut stack = Vec::with_capacity(1024);
        stack.push(StackFrame {
            node_idx: root_info.idx, stage: 0,
            has_child: root_info.has_child, has_sibling: root_info.has_sibling, made_move: false,
        });

        // スタックが空になるまで深さ優先でノードを評価する
        while let Some(mut frame) = stack.pop() {
            let curr_idx = frame.node_idx;
            let stage = frame.stage;
            let has_child = frame.has_child;
            let has_sibling = frame.has_sibling;

            // 行きがけ（stage 0）のノード処理を行う
            if stage == 0 {
                let px = self.nodes[curr_idx].x();
                let py = self.nodes[curr_idx].y();

                // 座標が盤面内に収まり、かつ空きマスであるか判定する
                let is_valid = px < 0 || {
                    // 座標が0〜14の範囲内かチェックする
                    if px >= 0 && px < 15 && py >= 0 && py < 15 {
                        let idx = (py as usize) * 15 + (px as usize);
                        unsafe { (*board.occupied.get_unchecked(idx / 64) & (1 << (idx % 64))) == 0 }
                    } else {
                        false
                    }
                };
                
                let mut applied = false;
                // 着手が有効であれば盤面に適用し、状態を記録する
                if is_valid {
                    board.make_move(px, py);
                    applied = true;
                    let hash = board.get_canonical_hash();
                    self.nodes[curr_idx].hash = hash;
                    self.nodes[curr_idx].set_depth(board.move_count as u8); 
                    
                    // HashMap ではなくフラットな配列に記録
                    flat_hashes.push((hash, curr_idx as u32));
                }

                frame.stage = 1;
                frame.made_move = applied;
                stack.push(frame);

                // 子ノードが存在する場合はパースしてスタックに積む
                if has_child {
                    if let Some(child_info) = self.read_node_from_slice(data, &mut offset) {
                        self.nodes[child_info.idx].parent = curr_idx as u32;
                        self.nodes[curr_idx].child = child_info.idx as u32;
                        stack.push(StackFrame {
                            node_idx: child_info.idx, stage: 0,
                            has_child: child_info.has_child, has_sibling: child_info.has_sibling, made_move: false,
                        });
                    }
                }
            // 帰りがけ（stage 1）のノード処理を行う
            } else if stage == 1 {
                // 行きがけで着手を適用していた場合は元に戻す
                if frame.made_move {
                    let px = self.nodes[curr_idx].x();
                    let py = self.nodes[curr_idx].y();
                    board.undo_move(px, py);
                }
                // 兄弟ノードが存在する場合はパースしてスタックに積む
                if has_sibling {
                    if let Some(sib_info) = self.read_node_from_slice(data, &mut offset) {
                        let p_idx = self.nodes[curr_idx].parent;
                        self.nodes[sib_info.idx].parent = p_idx;
                        self.nodes[curr_idx].sibling = sib_info.idx as u32;
                        stack.push(StackFrame {
                            node_idx: sib_info.idx, stage: 0,
                            has_child: sib_info.has_child, has_sibling: sib_info.has_sibling, made_move: false,
                        });
                    }
                }
            }
        } // while let Some(mut frame) = stack.pop() の終了

        // ループ終了後に一気に並列ソート＆重複排除して HashMap を構築
        #[cfg(not(target_arch = "wasm32"))]
        {
            flat_hashes.par_sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
        }
        #[cfg(target_arch = "wasm32")]
        {
            flat_hashes.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
        }

        // 最初の要素（＝idxが最も大きい最新のノード）だけを残して重複を排除
        flat_hashes.dedup_by_key(|k| k.0);
        
        // メモリのピーク消費を抑えるため、余分なキャパシティを解放
        flat_hashes.shrink_to_fit();

        // 重複排除されて小さくなったサイズで無駄なく確保して一気に挿入
        self.hash_table.reserve(flat_hashes.len());
        for (hash, idx) in flat_hashes {
            self.hash_table.insert(hash, idx);
        }

        Ok(())
    }

    // .dbファイルをストリームから直接パースする（メモリ消費最小化）
    pub fn parse_db_from_reader<R: Read>(&mut self, reader: R) -> io::Result<()> {
        let mut buf_reader = std::io::BufReader::with_capacity(64 * 1024, reader);
        
        // ストリームを消費せずに先頭のバッファを覗き見してLZ4マジックナンバーを確認
        let is_lz4 = {
            let buf = buf_reader.fill_buf()?;
            buf.len() >= 4 && u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) == 0x184D2204
        };

        if is_lz4 {
            let decoder = lz4_flex::frame::FrameDecoder::new(buf_reader);
            let decoder_buf = std::io::BufReader::with_capacity(64 * 1024, decoder);
            let mut ctx = ParserContext::new(decoder_buf);
            self.parse_db_inner(&mut ctx)
        } else {
            let mut ctx = ParserContext::new(buf_reader);
            self.parse_db_inner(&mut ctx)
        }
    }

    // .dbファイルをパースし、各種レコードデータを抽出する
    #[cfg(target_arch = "wasm32")]
    pub fn parse_db(&mut self, data: &[u8]) -> io::Result<()> {
        let is_lz4 = data.len() >= 4 && u32::from_le_bytes([data[0], data[1], data[2], data[3]]) == 0x184D2204;
        
        if is_lz4 {
            let cursor = std::io::Cursor::new(data);
            let decoder = lz4_flex::frame::FrameDecoder::new(cursor);
            let buf_reader = std::io::BufReader::with_capacity(64 * 1024, decoder);
            let mut ctx = ParserContext::new(buf_reader);
            self.parse_db_inner(&mut ctx)
        } else {
            let cursor = std::io::Cursor::new(data);
            let mut ctx = ParserContext::new(cursor);
            self.parse_db_inner(&mut ctx)
        }
    }

    fn parse_db_inner<R: BufRead>(&mut self, ctx: &mut ParserContext<R>) -> io::Result<()> {
        let num_records = ctx.read_u32_le()?;
            
            let limit = (num_records as usize).min(self.max_nodes);

            self.nodes.clear();
            self.nodes.reserve(limit + 1);
            self.hash_table.reserve(limit + 1);
            self.node_texts.resize(limit + 1, 0xFFFF | (0xFFFF << 16)); // 予め limit+1 個の空データ(0xFFFFFFFF)で埋めておく
            self.nodes.push(RenLibNode::new(-1, -1, NO_NODE, NO_NODE, NO_NODE, 0, 0));
            
            // 仮想ルート(Index 0)をハッシュ0として確実にテーブルに登録する
            self.hash_table.insert(0, 0);

            let mut hash_nexts = Vec::with_capacity(limit + 1);
            hash_nexts.push(NO_NODE);

            let mut node_stones_data = Vec::<u8>::with_capacity((limit * 10).min(10_000_000));
            let mut node_stones_index = Vec::<u32>::with_capacity(limit + 2);
            node_stones_index.push(0);
            node_stones_index.push(0);

            // 親ノードID, X座標, Y座標, テキストID のフラットなリスト
            let mut all_btxt: Vec<(usize, i8, i8, u32)> = Vec::with_capacity(limit / 10);
            let mut temp_stones: Vec<u8> = Vec::with_capacity(225);

            let mut percent_text_ids = [NO_TEXT; 100];
            // 1%から99%までの勝率文字列を事前確保する
            for i in 1..100 {
                let s = format!("{}%", i);
                percent_text_ids[i] = self.text_pool.get_or_insert(&self.encoding.encode(&s).0);
            }
            let id_w = self.text_pool.get_or_insert(&self.encoding.encode("W").0);
            let id_l = self.text_pool.get_or_insert(&self.encoding.encode("L").0);

            // 1手から225手までの詰み手数字列を事前確保する
            let mut mate_w_ids = vec![NO_TEXT; 226];
            let mut mate_l_ids = vec![NO_TEXT; 226];
            for i in 1..=225 {
                mate_w_ids[i] = self.text_pool.get_or_insert(&self.encoding.encode(&format!("W{}", i)).0);
                mate_l_ids[i] = self.text_pool.get_or_insert(&self.encoding.encode(&format!("L{}", i)).0);
            }

            // テキストやコメントの抽出で使い回すためのバッファ
            let mut text_buffer = Vec::with_capacity(512);

            // DB内の各レコードを順次解析する
            for _ in 0..num_records {
                // 読み込んだノード数が最大許容数に達したか判定する
                if self.nodes.len() >= self.max_nodes {
                    break;
                }

                // キー長の読み取りに成功したか検証し、失敗ならループを抜ける
                let num_key_bytes = match ctx.read_u16_le() { Ok(v) => v, Err(_) => break };
                // キー長が0の場合はスキップする
                if num_key_bytes == 0 { continue; }
                // キーバッファの取得に成功したか検証し、失敗ならループを抜ける
                let key_buffer = match ctx.read_slice(num_key_bytes as usize) { Ok(s) => s, Err(_) => break };
                
                let mut board = Board::new();
                let num_stones = (num_key_bytes as i32 - 3) / 2;
                temp_stones.clear();
                
                // 石データが存在する場合、盤面に配置して状態を構築する
                if num_stones >= 0 {
                    let num_black = (num_stones as f32 / 2.0).ceil() as usize;
                    // 黒石を指定座標に順次配置する
                    for k in 0..num_black {
                        let bx = key_buffer[3 + k * 2] as i8;
                        let by = key_buffer[4 + k * 2] as i8;
                        // 座標が有効であれば着手を盤面に適用する
                        if bx != -1 && by != -1 {
                            board.make_move_with_color(bx, by, 0);
                            temp_stones.push((by as u8) * 15 + (bx as u8));
                        }
                    }
                    
                    let num_white = (num_stones as f32 / 2.0).floor() as usize;
                    // 白石を指定座標に順次配置する
                    for k in 0..num_white {
                        let wx = key_buffer[3 + num_black * 2 + k * 2] as i8;
                        let wy = key_buffer[4 + num_black * 2 + k * 2] as i8;
                        // 座標が有効であれば着手を盤面に適用する
                        if wx != -1 && wy != -1 {
                            board.make_move_with_color(wx, wy, 1);
                            temp_stones.push((wy as u8) * 15 + (wx as u8));
                        }
                    }
                    
                    let hash = board.get_canonical_hash();
                    
                    // レコードバイト数の読み取りに成功したか検証し、失敗ならループを抜ける
                    let num_record_bytes = match ctx.read_u16_le() { Ok(v) => v, Err(_) => break };
                    // レコードバッファの取得に成功したか検証し、失敗ならループを抜ける
                    let record_buffer = match ctx.read_slice(num_record_bytes as usize) { Ok(s) => s, Err(_) => break };
                    
                    let mut text_id: u32 = NO_TEXT;
                    let mut comment_id: u32 = NO_TEXT;
                    
                    // レコードが5バイト以上で評価値情報が含まれているか確認する
                    if num_record_bytes >= 5 {
                        let label = record_buffer[0];
                        let value = i16::from_le_bytes([record_buffer[1], record_buffer[2]]);
                        let value_mate = 30000;
                        let value_mate_threshold = 29500;
                        let abs_val = value.abs();
                        
                        // 評価値が詰みの閾値を超えているか判定する
                        if abs_val > value_mate_threshold {
                            let steps = (value_mate - abs_val + 1) as usize;
                            if steps <= 225 {
                                text_id = if value < 0 { mate_w_ids[steps] } else { mate_l_ids[steps] };
                            } else {
                                let mate_text = if value < 0 { format!("W{}", steps) } else { format!("L{}", steps) };
                                text_id = self.text_pool.get_or_insert(&self.encoding.encode(&mate_text).0);
                            }
                        // ラベルが'W'（白勝ち）であるか判定する
                        } else if label == b'W' { text_id = id_w; 
                        // ラベルが'L'（黒勝ち）であるか判定する
                        } else if label == b'L' { text_id = id_l; 
                        // 評価値が0以外で勝率計算が必要か判定する
                        } else if value != 0 {
                            let k = 250.0;
                            let win_rate = 1.0 / (1.0 + (- (value as f64) / k).exp());
                            let win_percent = (win_rate * 100.0).floor() as i32;
                            // 勝率に応じたテキストID（100%、0%、またはその間の値）を決定する
                            if win_percent >= 100 { text_id = id_w; } else if win_percent <= 0 { text_id = id_l; } else { text_id = percent_text_ids[win_percent as usize]; }
                        }
                          
                        let text_data = &record_buffer[5..];
                        // テキストデータが"@BTXT@"マーカーから始まるか判定する
                        if text_data.starts_with(b"@BTXT@") {
                            let btxt_end = text_data.iter().position(|&b| b == 0x08).unwrap_or(text_data.len());
                            let mut idx = 6;
                            // 改行コードが存在する場合はスキップする
                            if idx < btxt_end && text_data[idx] == b'\n' { idx += 1; }
                            
                            // 文字を16進数の座標へ変換する
                            fn hex_to_coord(c: u8) -> i8 {
                                // 指定された文字の範囲に応じて16進数の値を返す
                                match c {
                                    b'0'..=b'9' => (c - b'0') as i8,
                                    b'A'..=b'F' => (c - b'A' + 10) as i8,
                                    b'a'..=b'f' => (c - b'a' + 10) as i8,
                                    _ => -1,
                                }
                            }
                            
                            let p_idx = self.nodes.len();
                            // 区切り文字までの間、BTXTの各座標・テキストペアを解析する
                            while idx < btxt_end {
                                // 座標データ（2文字）が残っているか確認する
                                if idx + 2 > btxt_end { break; }
                                let lx = hex_to_coord(text_data[idx]);
                                let ly = hex_to_coord(text_data[idx + 1]);
                                // 無効な座標文字が含まれている場合はスキップする
                                if lx < 0 || ly < 0 { idx += 2; continue; }
                                idx += 2;
                                let text_start = idx;
                                // 改行文字までテキストの末尾を探索する
                                while idx < btxt_end && text_data[idx] != b'\n' { idx += 1; }
                                
                                text_buffer.clear();
                                for &b in &text_data[text_start..idx] {
                                    if b != 0 && b != b'\r' {
                                        text_buffer.push(b);
                                    }
                                }
                                // 次のデータのために改行をスキップする
                                if idx < btxt_end && text_data[idx] == b'\n' { idx += 1; }
                                let t_id = self.text_pool.get_or_insert(&text_buffer);
                                all_btxt.push((p_idx, lx, ly, t_id));
                            }
                            
                            // BTXTマーカー以降に通常のコメントデータが存在するか確認する
                            if btxt_end + 1 < text_data.len() {
                                let slice = &text_data[btxt_end + 1..];
                                let mut len = slice.len();
                                // "charset="指定が含まれる場合はその前までで切り詰める
                                if let Some(pos) = slice.windows(8).position(|w| w == b"charset=") { len = pos; }
                                
                                text_buffer.clear();
                                for &b in &slice[..len] {
                                    if b != 0 && b != b'\r' { text_buffer.push(b); }
                                }
                                // コメント文字列が空でなければプールに登録する
                                if !text_buffer.is_empty() { comment_id = self.text_pool.get_or_insert(&text_buffer); }
                            }
                        // BTXTマーカーが存在しない通常のコメントデータを処理する
                        } else {
                             let mut slice = text_data;
                             // 区切り文字（0x08）が含まれる場合はそれ以降を抽出する
                             if let Some(pos) = slice.iter().position(|&b| b == 0x08) {
                                 slice = &slice[pos + 1..];
                             }
                             let mut len = slice.len();
                             // "charset="指定が含まれる場合はその前までで切り詰める
                             if let Some(pos) = slice.windows(8).position(|w| w == b"charset=") { len = pos; }
                             
                             text_buffer.clear();
                             for &b in &slice[..len] {
                                 if b != 0 && b != b'\r' { text_buffer.push(b); }
                             }
                             // コメント文字列が空でなければプールに登録する
                             if !text_buffer.is_empty() { comment_id = self.text_pool.get_or_insert(&text_buffer); }
                        }
                    }
                    
                    // 石データもテキストデータも持たない無意味なノードは追加せずスキップする
                    let has_btxt = all_btxt.last().map_or(false, |&(idx, ..)| idx == self.nodes.len());
                    if num_stones == 0 && comment_id == NO_TEXT && text_id == NO_TEXT && !has_btxt {
                        continue;
                    }

                    // 初期盤面(hash==0)のレコードは、新規追加せずルート(Index 0)のテキストに統合する
                    if hash == 0 {
                        if comment_id != NO_TEXT || text_id != NO_TEXT {
                            let cid = std::cmp::min(comment_id, 0xFFFF);
                            let tid = std::cmp::min(text_id, 0xFFFF);
                            let existing = self.node_texts[0];
                            let mut ex_cid = existing & 0xFFFF;
                            let mut ex_tid = (existing >> 16) & 0xFFFF;
                            
                            if cid != 0xFFFF { ex_cid = cid; }
                            if tid != 0xFFFF { ex_tid = tid; }
                            self.node_texts[0] = ex_cid | (ex_tid << 16);
                        }
                        continue; 
                    }
                    
                    node_stones_data.extend_from_slice(&temp_stones);
                    node_stones_index.push(node_stones_data.len() as u32);
                    
                    let idx = self.nodes.len();
                    self.nodes.push(RenLibNode::new(-1, -1, NO_NODE, NO_NODE, NO_NODE, hash, num_stones as u8));
                    
                    if comment_id != NO_TEXT || text_id != NO_TEXT {
                        let cid = std::cmp::min(comment_id, 0xFFFF);
                        let tid = std::cmp::min(text_id, 0xFFFF);
                        let texts = cid | (tid << 16);
                        
                        // limit を超えて push される場合への安全対策
                        if idx < self.node_texts.len() {
                            self.node_texts[idx] = texts;
                        } else {
                            self.node_texts.push(texts);
                        }
                    }
                    
                    let head = self.hash_table.get(&hash).copied().unwrap_or(NO_NODE);
                    hash_nexts.push(head);
                    self.hash_table.insert(hash, idx as u32);
                    
                // 石データが存在しない場合はレコードの長さ分スキップする
                } else {
                    // レコードバイト数の読み取りに成功したか検証し、失敗ならループを抜ける
                    let num_record_bytes = match ctx.read_u16_le() { Ok(v) => v, Err(_) => break };
                    let _ = ctx.skip(num_record_bytes as usize);
                }
            }

            let absolute_t = self.reconstruct_tree_optimized(&node_stones_data, &node_stones_index, &hash_nexts);
            
            // 1. 親インデックス順にソートする (unstableソートで十分かつ高速)
            all_btxt.sort_unstable_by_key(|&(p_idx, ..)| p_idx);

            // 2. 同じ親(p_idx)を持つ連続した要素をチャンクとしてまとめて処理する
            for chunk in all_btxt.chunk_by(|a, b| a.0 == b.0) {
                let p_idx = chunk[0].0; // チャンク内のすべての要素は同じ p_idx を持つ

                let mut history = Vec::new();
                let mut curr = p_idx as u32;
                // 親を辿って初期状態からの着手履歴を構築する
                while curr != NO_NODE {
                    let n = &self.nodes[curr as usize];
                    // 有効な着手座標であれば履歴に追加する
                    if n.x() >= 0 && n.y() >= 0 { history.push((n.x(), n.y())); }
                    curr = n.parent;
                }
                history.reverse();
                
                let mut pb = Board::new();
                // 履歴をもとに盤面状態を復元する（※同じ親に対しては1回だけで済む）
                for &(hx, hy) in &history { pb.make_move(hx, hy); }

                // チャンク内の各BTXTラベルを処理する
                for &(_, lx, ly, t_id) in chunk {
                    let root_idx = Zobrist::get_transformed_idx(lx, ly, absolute_t[p_idx] as usize);
                    let root_lx = (root_idx % 15) as i8;
                    let root_ly = (root_idx / 15) as i8;

                    pb.make_move(root_lx, root_ly);
                    let target_hash = pb.get_canonical_hash();
                    pb.undo_move(root_lx, root_ly);

                    let mut curr = self.nodes[p_idx].child;
                    // 親に属する子ノードを走査して対象ハッシュを検索する
                    while curr != NO_NODE {
                        // 子ノードのハッシュがターゲットと一致すればテキストIDを設定して探索を終える
                        if self.nodes[curr as usize].hash == target_hash {
                            let tid = std::cmp::min(t_id, 0xFFFF);
                            // idx は配列の範囲内であることが保証されているため、直接アクセス
                            let texts = &mut self.node_texts[curr as usize];
                            *texts = (*texts & 0x0000FFFF) | (tid << 16);
                            break;
                        }
                        curr = self.nodes[curr as usize].sibling;
                    }
                }
            }

        Ok(())
    }



    // 独立した石データから親子関係と対称性を解決して木構造を再構築する
    pub fn reconstruct_tree_optimized(&mut self, node_stones_data: &[u8], node_stones_index: &[u32], hash_nexts: &[u32]) -> Vec<u8> {
        let num_nodes = self.nodes.len();
        let mut absolute_t = vec![0u8; num_nodes];
        // ノードがルートのみなど少なすぎる場合はそのまま返す
        if num_nodes < 2 { return absolute_t; }

        let mut local_rx = vec![0i8; num_nodes];
        let mut local_ry = vec![0i8; num_nodes];
        let mut edge_inv_t = vec![0u8; num_nodes];

        // --- 1. 共有データの抽出（読み取り専用参照の作成） ---
        // Rayonのクロージャ内で self 全体をキャプチャすると借用エラーになるため、
        // 必要なフィールドだけを独立した不変参照として切り出します。
        let nodes_ref = &self.nodes;
        let hash_table_ref = &self.hash_table;

        // --- 2. Mapフェーズ用の純粋関数（クロージャ） ---
        let map_func = |i: usize| -> Option<ParentSearchResult> {
            let depth_b = nodes_ref[i].depth();
            if depth_b == 0 { return None; } // 深さが0（初期状態）はスキップ

            let start_idx = node_stones_index[i] as usize;
            let end_idx = if i + 1 < node_stones_index.len() { node_stones_index[i + 1] as usize } else { node_stones_data.len() };
            let stones_b = if start_idx < end_idx && end_idx <= node_stones_data.len() {
                &node_stones_data[start_idx..end_idx]
            } else {
                &[]
            };

            let num_black_total = (depth_b as f32 / 2.0).ceil() as usize;
            let mut temp_board = Board::new();
            // 自身の石をすべて盤面に配置してハッシュリストを生成する
            for (k, &val) in stones_b.iter().enumerate() {
                let sx = (val % 15) as i8;
                let sy = (val / 15) as i8;
                let color = if k < num_black_total { 0 } else { 1 }; 
                temp_board.make_move_with_color(sx, sy, color);
            }
            let hashes_b = temp_board.hashes;
            
            let color_to_remove = (depth_b - 1) % 2;
            let num_black = (depth_b as f32 / 2.0).ceil() as usize;

            // 直前に置かれた可能性のある石（同色）のリストを決定する
            let stones_to_check = if color_to_remove == 0 {
                &stones_b[0..num_black.min(stones_b.len())]
            } else {
                if num_black <= stones_b.len() { &stones_b[num_black..] } else { &[] }
            };

            // 候補となる石を1つずつ取り除き、親のハッシュを探索する
            for &val in stones_to_check {
                let rx = (val % 15) as i8;
                let ry = (val / 15) as i8;
                let mut min_h = u64::MAX;
                let t_table = Zobrist::transformed_table();
                let cell_hashes = unsafe { t_table.get_unchecked((ry as usize * 15) + rx as usize) };
                
                for k in 0..8 {
                    unsafe {
                        let h_p_sym = hashes_b.get_unchecked(k) ^ cell_hashes.get_unchecked(k).get_unchecked(color_to_remove as usize);
                        if h_p_sym < min_h { min_h = h_p_sym; }
                    }
                }

                // 算出した親の最小ハッシュがテーブルに存在するか確認する
                if let Some(&head_u32) = hash_table_ref.get(&min_h) {
                    let mut p_idx_u32 = head_u32;
                    let mut valid_p_idx = NO_NODE;
                    
                    // チェインを辿り、深さが「自分の深さ - 1」のノードを正しい親として選択する
                    while p_idx_u32 != NO_NODE {
                        if nodes_ref[p_idx_u32 as usize].depth() == depth_b - 1 {
                            valid_p_idx = p_idx_u32;
                            break;
                        }
                        p_idx_u32 = hash_nexts[p_idx_u32 as usize];
                    }
                    
                    // 正しい親が見つかった場合のみリンク処理を行う
                    if valid_p_idx != NO_NODE {
                        let p_idx = valid_p_idx as usize;
                        let target_hash = hashes_b[0] ^ cell_hashes[0][color_to_remove as usize];

                        let p_depth = nodes_ref[p_idx].depth();
                        let p_num_black = (p_depth as f32 / 2.0).ceil() as usize;
                        let p_start = node_stones_index[p_idx] as usize;
                        let p_end = if p_idx + 1 < node_stones_index.len() { node_stones_index[p_idx + 1] as usize } else { node_stones_data.len() };
                        let stones_p = &node_stones_data[p_start..p_end];
                        
                        let mut temp_board_p = Board::new();
                        for (k, &val) in stones_p.iter().enumerate() {
                            let sx = (val % 15) as i8;
                            let sy = (val / 15) as i8;
                            let color = if k < p_num_black { 0 } else { 1 };
                            temp_board_p.make_move_with_color(sx, sy, color);
                        }
                        let p_hashes = temp_board_p.hashes;

                        let mut best_t: usize = 0;
                        for t in 0..8 {
                            if p_hashes[t] == target_hash { best_t = t; break; }
                        }

                        let inv_t = [0, 3, 2, 1, 4, 5, 6, 7][best_t];
                        
                        // 副作用を起こさずに結果を返す
                        return Some(ParentSearchResult {
                            parent_idx: p_idx as u32,
                            inv_t: inv_t as u8,
                            rx,
                            ry,
                        });
                    }
                }
            }

            // 深さ1で親が見つからなかった場合、強制的にルートに直結させる
            if depth_b == 1 && !stones_b.is_empty() {
                let val = stones_b[0];
                let rx = (val % 15) as i8;
                let ry = (val / 15) as i8;
                return Some(ParentSearchResult {
                    parent_idx: 0,
                    inv_t: 0,
                    rx,
                    ry,
                });
            }
            
            None
        };

        // --- 3. 並列 Map フェーズ ---
        // Wasm環境とネイティブ環境でコンパイルを分岐させます
        #[cfg(not(target_arch = "wasm32"))]
        let results: Vec<Option<ParentSearchResult>> = (1..num_nodes).into_par_iter().map(map_func).collect();

        #[cfg(target_arch = "wasm32")]
        let results: Vec<Option<ParentSearchResult>> = (1..num_nodes).map(map_func).collect();

        // --- 4. 逐次 Reduce / Apply フェーズ ---
        // O(1)で末尾に兄弟を追加するためのキャッシュ配列
        let mut last_child = vec![NO_NODE; num_nodes]; 

        // ここで初めて self.nodes を可変借用し、安全にリンクを繋ぎます
        for (i_minus_1, res) in results.into_iter().enumerate() {
            let i = i_minus_1 + 1;
            if let Some(r) = res {
                let p_idx = r.parent_idx as usize;
                
                self.nodes[i].parent = r.parent_idx;
                edge_inv_t[i] = r.inv_t;
                local_rx[i] = r.rx;
                local_ry[i] = r.ry;

                // 兄弟リンクの構築を O(1) 化
                if self.nodes[p_idx].child == NO_NODE {
                    // 子が存在しない場合は最初の子として登録する
                    self.nodes[p_idx].child = i as u32;
                } else {
                    // すでに子がいる場合は末尾（last_child）の兄弟リンクに繋ぐ
                    let last = last_child[p_idx] as usize;
                    self.nodes[last].sibling = i as u32;
                }
                // 自分が「最後の子」になるため記録を更新する
                last_child[p_idx] = i as u32;
            }
        }

        // --- 5. 絶対座標確定フェーズ（逐次処理） ---
        let mut stack = vec![0usize];
        // 構築した木をルートから辿り、各ノードの絶対的な座標変換インデックスを計算する
        while let Some(curr) = stack.pop() {
            let mut child_opt = self.nodes[curr].child;
            // 全ての子ノードに対して対称性に基づく絶対座標を適用する
            while child_opt != NO_NODE {
                let c_idx = child_opt as usize;
                let t_to_root = Zobrist::compose_t(edge_inv_t[c_idx] as usize, absolute_t[curr] as usize);
                absolute_t[c_idx] = t_to_root as u8;

                let rx = local_rx[c_idx];
                let ry = local_ry[c_idx];
                
                // 座標が負（パス）の場合はそのまま設定する
                if rx < 0 || ry < 0 {
                    self.nodes[c_idx].set_x(-1);
                    self.nodes[c_idx].set_y(-1);
                } else {
                    // 正規の座標の場合は変換を行って設定する
                    let root_idx = Zobrist::get_transformed_idx(rx, ry, t_to_root);
                    self.nodes[c_idx].set_x((root_idx % 15) as i8);
                    self.nodes[c_idx].set_y((root_idx / 15) as i8);
                }

                stack.push(c_idx);
                child_opt = self.nodes[c_idx].sibling;
            }
        }

        absolute_t
    }
}
