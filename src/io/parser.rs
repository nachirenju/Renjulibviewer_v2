use std::io::{self, Cursor, Read};
use rustc_hash::FxHashMap;
use encoding_rs::SHIFT_JIS;

use crate::core::node::*;
use crate::core::zobrist::Zobrist;
use crate::core::hash_board::Board;

// バイト列のパース状態を管理する構造体を定義する
struct ParserContext<'a> {
    data: &'a [u8],
    pos: usize,
}

// ParserContextに対するメソッド群を実装する
impl<'a> ParserContext<'a> {
    // 1バイトのデータを読み込む処理を行う
    #[inline(always)]
    fn read_u8(&mut self) -> io::Result<u8> {
        // 現在の読み込み位置がデータ終端に達していないか確認する
        if self.pos < self.data.len() {
            let val = self.data[self.pos];
            self.pos += 1;
            Ok(val)
        } else {
            // データ不足の場合はEOFエラーを返す
            Err(io::Error::new(io::ErrorKind::UnexpectedEof, "EOF"))
        }
    }

    // リトルエンディアンで2バイトの数値を読み込む
    #[inline(always)]
    fn read_u16_le(&mut self) -> io::Result<u16> {
        // 2バイト分のデータが残っているか確認する
        if self.pos + 2 <= self.data.len() {
            let val = u16::from_le_bytes([self.data[self.pos], self.data[self.pos+1]]);
            self.pos += 2;
            Ok(val)
        } else {
            // データ不足の場合はEOFエラーを返す
            Err(io::Error::new(io::ErrorKind::UnexpectedEof, "EOF"))
        }
    }

    // リトルエンディアンで4バイトの数値を読み込む
    #[inline(always)]
    fn read_u32_le(&mut self) -> io::Result<u32> {
        // 4バイト分のデータが残っているか確認する
        if self.pos + 4 <= self.data.len() {
            let val = u32::from_le_bytes([
                self.data[self.pos], self.data[self.pos+1],
                self.data[self.pos+2], self.data[self.pos+3]
            ]);
            self.pos += 4;
            Ok(val)
        } else {
            // データ不足の場合はEOFエラーを返す
            Err(io::Error::new(io::ErrorKind::UnexpectedEof, "EOF"))
        }
    }

    // 指定されたバイト数だけ読み込み位置をスキップする
    #[inline(always)]
    fn skip(&mut self, n: usize) -> io::Result<()> {
        // スキップ分のデータが存在するか確認する
        if self.pos + n <= self.data.len() {
            self.pos += n;
            Ok(())
        } else {
            // データ不足の場合はEOFエラーを返す
            Err(io::Error::new(io::ErrorKind::UnexpectedEof, "EOF"))
        }
    }

    // 指定された長さのバイトスライスを取得する
    #[inline(always)]
    fn read_slice(&mut self, len: usize) -> io::Result<&'a [u8]> {
        // 要求された長さのデータが存在するか確認する
        if self.pos + len <= self.data.len() {
            let slice = &self.data[self.pos .. self.pos + len];
            self.pos += len;
            Ok(slice)
        } else {
            // データ不足の場合はEOFエラーを返す
            Err(io::Error::new(io::ErrorKind::UnexpectedEof, "EOF"))
        }
    }

    // ヌル文字（0x00）までのバイトスライスを取得する
    #[inline(always)]
    fn read_raw_bytes_slice(&mut self) -> io::Result<&'a [u8]> {
        let remaining = &self.data[self.pos..];
        // 残りのバイト列からヌル文字の位置を探索する
        if let Some(null_pos) = remaining.iter().position(|&b| b == 0) {
            let slice = &self.data[self.pos .. self.pos + null_pos];
            let new_pos = self.pos + null_pos + 1;
            // 2バイト境界に合わせるためのパディング計算を行う
            let final_pos = if (slice.len() + 1) % 2 != 0 { new_pos + 1 } else { new_pos };
            self.pos = final_pos.min(self.data.len());
            Ok(slice)
        } else {
            // ヌル文字が見つからない場合はEOFエラーを返す
            Err(io::Error::new(io::ErrorKind::UnexpectedEof, "EOF"))
        }
    }
}

// 棋譜ファイルのパースおよび木構造構築を行う構造体を定義する
pub struct RenLibParser {
    pub nodes: Vec<RenLibNode>,
    pub hash_table: FxHashMap<u64, u32>, 
    pub max_nodes: usize,
    encoding: &'static encoding_rs::Encoding,
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

// RenLibParserに対するメソッド群を実装する
impl RenLibParser {
    // RenLibParserの初期化を行う
    pub fn new(max_nodes: usize) -> Self {
        Self {
            nodes: Vec::with_capacity(20000),
            hash_table: FxHashMap::default(), 
            max_nodes,
            encoding: SHIFT_JIS,
            text_pool: TextPool::new(),
        }
    }

    // 文字エンコーディングを設定する
    pub fn set_encoding(&mut self, encoding: &'static encoding_rs::Encoding) {
        self.encoding = encoding;
    }

    // 1ノード分の情報をパースし、ノードリストに追加する
    fn read_node_to_pool(&mut self, ctx: &mut ParserContext) -> io::Result<NodeInfo> {
        // 設定された最大ノード数を超過していないか確認する
        if self.nodes.len() >= self.max_nodes {
            return Err(io::Error::new(io::ErrorKind::Other, "NODE_LIMIT_REACHED"));
        }

        let move_byte = ctx.read_u8()?;
        let flag = ctx.read_u8()?;

        // バイト値からX座標とY座標を算出する
        let (x, y) = if move_byte == 0 {
            (-1, -1) 
        } else {
            (( (move_byte & 0x0f) as i8 - 1), ( (move_byte >> 4) as i8))
        };

        // テキストフラグが立っている場合、追加の2バイトを空読みする
        if (flag & MASK_TEXT) != 0 { let _ = ctx.read_u16_le()?; }

        // コメントフラグに基づくコメントIDの取得を行う
        let comment_id = if (flag & MASK_COMMENT) != 0 {
            let b = ctx.read_raw_bytes_slice()?;
            // バイト列が空かどうかに応じてプール登録またはNO_TEXTを返す
            if b.is_empty() { NO_TEXT } else { self.text_pool.get_or_insert(b) }
        } else {
            NO_TEXT
        };

        // テキストフラグに基づくテキストIDの取得を行う
        let text_id = if (flag & MASK_TEXT) != 0 {
            let b = ctx.read_raw_bytes_slice()?;
            // バイト列が空かどうかに応じてプール登録またはNO_TEXTを返す
            if b.is_empty() { NO_TEXT } else { self.text_pool.get_or_insert(b) }
        } else {
            NO_TEXT
        };

        let idx = self.nodes.len();
        self.nodes.push(RenLibNode::new(
            x, y, comment_id, text_id, NO_NODE, NO_NODE, NO_NODE, NO_NODE, 0, 0
        ));

        Ok(NodeInfo {
            idx,
            has_child: (flag & MASK_NOCHILD) == 0,
            has_sibling: (flag & MASK_SIBLING) != 0,
        })
    }

    // .libファイルの全データから木構造を構築する
    pub fn parse_all(&mut self, data: &[u8], board: &mut Board) -> io::Result<()> {
        let mut ctx = ParserContext { data, pos: 0 };
        ctx.skip(20)?;
        
        let estimated_nodes = (data.len() / 20).max(1000).min(self.max_nodes);
        self.nodes.reserve(estimated_nodes);
        self.hash_table.reserve(estimated_nodes);

        self.nodes.clear();
        self.nodes.push(RenLibNode::new(-1, -1, NO_TEXT, NO_TEXT, NO_NODE, NO_NODE, NO_NODE, NO_NODE, 0, 0));

        // ルートノードの読み込みを試行する
        let mut root_info = match self.read_node_to_pool(&mut ctx) {
            Ok(info) => info, Err(_) => return Ok(()),
        };

        // ルートノードがパス指定（座標が負）の場合の移譲処理を行う
        if self.nodes[root_info.idx].x() < 0 {
            let c_id = self.nodes[root_info.idx].comment_id();
            // コメントが存在すれば仮想ルートノードに引き継ぐ
            if c_id != NO_TEXT {
                self.nodes[0].set_comment_id(c_id);
                self.nodes[root_info.idx].set_comment_id(NO_TEXT);
            }
            let t_id = self.nodes[root_info.idx].text_id();
            // テキストが存在すれば仮想ルートノードに引き継ぐ
            if t_id != NO_TEXT {
                self.nodes[0].set_text_id(t_id);
                self.nodes[root_info.idx].set_text_id(NO_TEXT);
            }
            let has_child = root_info.has_child;
            self.nodes.pop(); 
            // 子ノードが存在すればそれを新しい実質ルートとしてパースする
            if has_child {
                // 新ルートの読み込みを試行する
                root_info = match self.read_node_to_pool(&mut ctx) {
                    Ok(info) => info, Err(_) => return Ok(()),
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
                    
                    let head = self.hash_table.get(&hash).copied().unwrap_or(NO_NODE);
                    self.nodes[curr_idx].hash_next = head;
                    self.hash_table.insert(hash, curr_idx as u32);
                }

                frame.stage = 1;
                frame.made_move = applied;
                stack.push(frame);

                // 子ノードが存在する場合はパースしてスタックに積む
                if has_child {
                    // 子ノードの情報を取得できた場合、リンクを構築する
                    if let Ok(child_info) = self.read_node_to_pool(&mut ctx) {
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
                    // 兄弟ノードの情報を取得できた場合、リンクを構築する
                    if let Ok(sib_info) = self.read_node_to_pool(&mut ctx) {
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
        }
        Ok(())
    }

    // .dbファイルをパースし、各種レコードデータを抽出する
    pub fn parse_db(&mut self, data: &[u8]) -> io::Result<()> {
        #[cfg(not(target_arch = "wasm32"))]
        let mut _temp_file_path = None;

        // DB展開と構文解析のブロックを開始する
        {
            #[cfg(not(target_arch = "wasm32"))]
            let mut _mmap_guard = None;
            let mut decompressed_buffer = Vec::new();
            let mut target_data = data;
            
            // データ長が4バイト以上あるか確認する
            if data.len() >= 4 {
                let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                // LZ4圧縮を示すマジックナンバーと一致するか検証する
                if magic == 0x184D2204 {
                    #[cfg(not(target_arch = "wasm32"))]
                    // WASM以外の環境向けLZ4解凍ブロック
                    {
                        let temp_name = format!("renju_lz4_temp_{}_{}.db", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos());
                        let temp_path = std::env::temp_dir().join(temp_name);
                        
                        // 一時ファイルの作成に成功したか判定する
                        if let Ok(mut temp_file) = std::fs::File::create(&temp_path) {
                            let mut decoder = lz4_flex::frame::FrameDecoder::new(Cursor::new(data));
                            // 解凍内容のファイルへの書き込みが成功したか判定する
                            if std::io::copy(&mut decoder, &mut temp_file).is_ok() {
                                // 書き込んだファイルを開き直せたか判定する
                                if let Ok(file_read) = std::fs::File::open(&temp_path) {
                                    // メモリマッピング（mmap）の構築が成功したか判定する
                                    if let Ok(mmap) = unsafe { memmap2::Mmap::map(&file_read) } {
                                        _mmap_guard = Some(mmap);
                                        _temp_file_path = Some(temp_path);
                                    }
                                }
                            }
                        }
                        
                        // mmapが利用できない場合のフォールバック処理を行う
                        if _mmap_guard.is_none() {
                            let mut decoder = lz4_flex::frame::FrameDecoder::new(Cursor::new(data));
                            // メモリバッファへの全展開が成功したか判定する
                            if decoder.read_to_end(&mut decompressed_buffer).is_ok() {
                                target_data = &decompressed_buffer;
                            } else {
                                return Err(io::Error::new(io::ErrorKind::InvalidData, "LZ4 decompression failed"));
                            }
                        } else {
                            target_data = _mmap_guard.as_ref().unwrap();
                        }
                    }
                    #[cfg(target_arch = "wasm32")]
                    // WASM環境向けLZ4解凍ブロック
                    {
                        let mut decoder = lz4_flex::frame::FrameDecoder::new(Cursor::new(data));
                        // メモリバッファへの全展開が成功したか判定する
                        if decoder.read_to_end(&mut decompressed_buffer).is_ok() {
                            target_data = &decompressed_buffer;
                        } else {
                            return Err(io::Error::new(io::ErrorKind::InvalidData, "LZ4 decompression failed"));
                        }
                    }
                }
            }

            let mut ctx = ParserContext { data: target_data, pos: 0 };
            let num_records = ctx.read_u32_le()?;
            
            let limit = (num_records as usize).min(self.max_nodes);

            self.nodes.clear();
            self.nodes.reserve(limit + 1);
            self.hash_table.reserve(limit + 1);
            self.nodes.push(RenLibNode::new(-1, -1, NO_TEXT, NO_TEXT, NO_NODE, NO_NODE, NO_NODE, NO_NODE, 0, 0));
            
            let mut node_stones_data = Vec::<(i8, i8)>::with_capacity((limit * 10).min(10_000_000));
            let mut node_stones_index = Vec::<u32>::with_capacity(limit + 2);
            node_stones_index.push(0);
            node_stones_index.push(0);

            let mut all_btxt: FxHashMap<usize, Vec<(i8, i8, u32)>> = FxHashMap::default();
            let mut temp_stones = Vec::with_capacity(225);

            let mut percent_text_ids = [NO_TEXT; 100];
            // 1%から99%までの勝率文字列を事前確保する
            for i in 1..100 {
                let s = format!("{}%", i);
                percent_text_ids[i] = self.text_pool.get_or_insert(&self.encoding.encode(&s).0);
            }
            let id_w = self.text_pool.get_or_insert(&self.encoding.encode("W").0);
            let id_l = self.text_pool.get_or_insert(&self.encoding.encode("L").0);

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
                            temp_stones.push((bx, by));
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
                            temp_stones.push((wx, wy));
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
                            let steps = value_mate - abs_val + 1;
                            let mate_text = if value < 0 { format!("W{}", steps) } else { format!("L{}", steps) };
                            text_id = self.text_pool.get_or_insert(&self.encoding.encode(&mate_text).0);
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
                            
                            let mut node_btxt = Vec::new();
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
                                
                                let mut txt_bytes = text_data[text_start..idx].to_vec();
                                txt_bytes.retain(|&b| b != 0 && b != b'\r');
                                // 次のデータのために改行をスキップする
                                if idx < btxt_end && text_data[idx] == b'\n' { idx += 1; }
                                let t_id = self.text_pool.get_or_insert(&txt_bytes);
                                node_btxt.push((lx, ly, t_id));
                            }
                            
                            // 抽出したBTXTデータが存在すれば、ノードのインデックスと紐付けて保存する
                            if !node_btxt.is_empty() { all_btxt.insert(self.nodes.len(), node_btxt); }
                            
                            // BTXTマーカー以降に通常のコメントデータが存在するか確認する
                            if btxt_end + 1 < text_data.len() {
                                let mut cb = text_data[btxt_end + 1..].to_vec();
                                // "charset="指定が含まれる場合はその前までで切り詰める
                                if let Some(pos) = cb.windows(8).position(|w| w == b"charset=") { cb.truncate(pos); }
                                cb.retain(|&b| b != 0 && b != b'\r');
                                // コメント文字列が空でなければプールに登録する
                                if !cb.is_empty() { comment_id = self.text_pool.get_or_insert(&cb); }
                            }
                        // BTXTマーカーが存在しない通常のコメントデータを処理する
                        } else {
                             let mut cb = text_data.to_vec();
                             // 区切り文字（0x08）が含まれる場合はそれ以降を抽出する
                             if let Some(pos) = cb.iter().position(|&b| b == 0x08) {
                                 cb = cb[pos + 1..].to_vec();
                             }
                             // "charset="指定が含まれる場合はその前までで切り詰める
                             if let Some(pos) = cb.windows(8).position(|w| w == b"charset=") { cb.truncate(pos); }
                             cb.retain(|&b| b != 0 && b != b'\r');
                             // コメント文字列が空でなければプールに登録する
                             if !cb.is_empty() { comment_id = self.text_pool.get_or_insert(&cb); }
                        }
                    }
                    
                    // 石データもテキストデータも持たない無意味なノードは追加せずスキップする
                    if num_stones == 0 && comment_id == NO_TEXT && text_id == NO_TEXT && !all_btxt.contains_key(&self.nodes.len()) {
                        continue;
                    }
                    
                    node_stones_data.extend_from_slice(&temp_stones);
                    node_stones_index.push(node_stones_data.len() as u32);
                    
                    let idx = self.nodes.len();
                    self.nodes.push(RenLibNode::new(-1, -1, comment_id, text_id, NO_NODE, NO_NODE, NO_NODE, NO_NODE, hash, num_stones as u8));
                    
                    let head = self.hash_table.get(&hash).copied().unwrap_or(NO_NODE);
                    self.nodes[idx].hash_next = head;
                    self.hash_table.insert(hash, idx as u32);
                    
                // 石データが存在しない場合はレコードの長さ分スキップする
                } else {
                    // レコードバイト数の読み取りに成功したか検証し、失敗ならループを抜ける
                    let num_record_bytes = match ctx.read_u16_le() { Ok(v) => v, Err(_) => break };
                    let _ = ctx.skip(num_record_bytes as usize);
                }
            }

            let absolute_t = self.reconstruct_tree_optimized(&node_stones_data, &node_stones_index);
            
            // 収集した各親ノードのBTXT情報を子ノードに割り当てる
            for (p_idx, labels) in all_btxt {
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
                // 履歴をもとに盤面状態を復元する
                for &(hx, hy) in &history { pb.make_move(hx, hy); }

                // 各BTXTラベルが指し示す着手ハッシュを計算し、子ノードにテキストを反映する
                for (lx, ly, t_id) in labels {
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
                            self.nodes[curr as usize].set_text_id(t_id);
                            break;
                        }
                        curr = self.nodes[curr as usize].sibling;
                    }
                }
            }
        } 

        #[cfg(not(target_arch = "wasm32"))]
        // 解凍に利用した一時ファイルが残っていれば削除する
        if let Some(path) = _temp_file_path {
            let _ = std::fs::remove_file(path);
        }
        
        Ok(())
    }

    // 独立した石データから親子関係と対称性を解決して木構造を再構築する
    pub fn reconstruct_tree_optimized(&mut self, node_stones_data: &[(i8, i8)], node_stones_index: &[u32]) -> Vec<u8> {
        
        let mut absolute_t = vec![0u8; self.nodes.len()];
        // ノードがルートのみなど少なすぎる場合はそのまま返す
        if self.nodes.len() < 2 { return absolute_t; }

        let mut local_rx = vec![0i8; self.nodes.len()];
        let mut local_ry = vec![0i8; self.nodes.len()];
        let mut edge_inv_t = vec![0u8; self.nodes.len()];

        // 各ノードについて、最も適切な親ノードをハッシュから逆算する
        for i in 1..self.nodes.len() {
            let depth_b = self.nodes[i].depth();
            
            let start_idx = node_stones_index[i] as usize;
            let end_idx = if i + 1 < node_stones_index.len() { node_stones_index[i + 1] as usize } else { node_stones_data.len() };
            // 石データの抽出範囲が有効であるか判定し、該当するスライスを取得する
            let stones_b = if start_idx < end_idx && end_idx <= node_stones_data.len() {
                &node_stones_data[start_idx..end_idx]
            } else {
                &[]
            };

            // 深さが0（初期状態）のノードは親探索をスキップする
            if depth_b == 0 { continue; }

            let num_black_total = (depth_b as f32 / 2.0).ceil() as usize;
            let mut temp_board = Board::new();
            // 自身の石をすべて盤面に配置してハッシュリストを生成する
            for (k, &(sx, sy)) in stones_b.iter().enumerate() {
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
                // 白石の範囲が有効であるか判定する
                if num_black <= stones_b.len() { &stones_b[num_black..] } else { &[] }
            };

            // 候補となる石を1つずつ取り除き、親のハッシュを探索する
            for &(rx, ry) in stones_to_check {
                let mut min_h = u64::MAX;
                let t_table = Zobrist::transformed_table();
                let cell_hashes = unsafe { t_table.get_unchecked((ry as usize * 15) + rx as usize) };
                
                // 8つの対称形ハッシュ群から代表となる最小ハッシュ値を算出する
                for k in 0..8 {
                    unsafe {
                        let h_p_sym = hashes_b.get_unchecked(k) ^ cell_hashes.get_unchecked(k).get_unchecked(color_to_remove as usize);
                        if h_p_sym < min_h { min_h = h_p_sym; }
                    }
                }

                // 算出した親の最小ハッシュがテーブルに存在するか確認する
                if let Some(&head_u32) = self.hash_table.get(&min_h) {
                    let mut p_idx_u32 = head_u32;
                    // 同一ハッシュの中で最初に登録された（最も古い）ノードまで遡る
                    while self.nodes[p_idx_u32 as usize].hash_next != NO_NODE {
                        p_idx_u32 = self.nodes[p_idx_u32 as usize].hash_next;
                    }
                    let p_idx = p_idx_u32 as usize;
                    
                    self.nodes[i].parent = p_idx as u32;
                    
                    // 親に既に子が登録されているか確認し、末尾の兄弟として追加する
                    if self.nodes[p_idx].child != NO_NODE {
                        let mut curr = self.nodes[p_idx].child as usize;
                        // 末尾の兄弟ノードを探す
                        while self.nodes[curr].sibling != NO_NODE {
                            curr = self.nodes[curr].sibling as usize;
                        }
                        self.nodes[curr].sibling = i as u32;
                    } else {
                        // 子が存在しない場合は最初の子として登録する
                        self.nodes[p_idx].child = i as u32;
                    }

                    let target_hash = hashes_b[0] ^ cell_hashes[0][color_to_remove as usize];

                    let p_depth = self.nodes[p_idx].depth();
                    let p_num_black = (p_depth as f32 / 2.0).ceil() as usize;
                    let p_start = node_stones_index[p_idx] as usize;
                    let p_end = if p_idx + 1 < node_stones_index.len() { node_stones_index[p_idx + 1] as usize } else { node_stones_data.len() };
                    let stones_p = &node_stones_data[p_start..p_end];
                    
                    let mut temp_board_p = Board::new();
                    // 特定された親の石を盤面に配置してハッシュリストを生成する
                    for (k, &(sx, sy)) in stones_p.iter().enumerate() {
                        let color = if k < p_num_black { 0 } else { 1 };
                        temp_board_p.make_move_with_color(sx, sy, color);
                    }
                    let p_hashes = temp_board_p.hashes;

                    let mut best_t: usize = 0;
                    // 親の8つの対称ハッシュのうち、取り除き計算したハッシュと一致するものを探す
                    for t in 0..8 {
                        if p_hashes[t] == target_hash { best_t = t; break; }
                    }

                    let inv_t = [0, 3, 2, 1, 4, 5, 6, 7][best_t];
                    edge_inv_t[i] = inv_t as u8;
                    local_rx[i] = rx;
                    local_ry[i] = ry;
                    break;
                }
            }

            // 深さ1で親が見つからなかった場合、強制的にルートに直結させる
            if self.nodes[i].parent == NO_NODE && depth_b == 1 && !stones_b.is_empty() {
                let (rx, ry) = stones_b[0];
                let p_idx = 0;
                self.nodes[i].parent = p_idx as u32;
                
                // ルートに既に子が登録されているか確認し、末尾の兄弟として追加する
                if self.nodes[p_idx].child != NO_NODE {
                    let mut curr = self.nodes[p_idx].child as usize;
                    // 末尾の兄弟ノードを探す
                    while self.nodes[curr].sibling != NO_NODE { curr = self.nodes[curr].sibling as usize; }
                    self.nodes[curr].sibling = i as u32;
                } else {
                    // ルートに子が存在しない場合は最初の子として登録する
                    self.nodes[p_idx].child = i as u32;
                }
                
                edge_inv_t[i] = 0;
                local_rx[i] = rx;
                local_ry[i] = ry;
            }
        }

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
