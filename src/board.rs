// 連珠盤の論理的な状態管理、石の配置、および履歴管理を行う
pub const SIZE: usize = 15;

#[derive(Clone, Copy, PartialEq)]
pub enum Player {
    Black,
    White,
}

pub struct Board {
    pub grid: [Option<Player>; SIZE * SIZE],
    pub next_player: Player,
    pub history: Vec<usize>, // 履歴を追加
    pub future_history: Vec<usize>, // 取り消した手の履歴
}

impl Board {
    pub fn new() -> Self {
        Self {
            grid: [None; SIZE * SIZE],
            next_player: Player::Black,
            history: Vec::new(),
            future_history: Vec::new(),
        }
    }

    pub fn place_stone(&mut self, x: usize, y: usize) -> bool {
        let idx = y * SIZE + x;
        if x < SIZE && y < SIZE && self.grid[idx].is_none() {
            self.grid[idx] = Some(self.next_player);
            self.history.push(idx); // 着手時にインデックスを保存
            self.future_history.clear(); // 新しく着手したら未来の履歴をクリア
            self.next_player = match self.next_player {
                Player::Black => Player::White,
                Player::White => Player::Black,
            };
            return true;
        }
        false
    }

    pub fn undo(&mut self) {
        if let Some(last_idx) = self.history.pop() {
            self.grid[last_idx] = None;
            self.future_history.push(last_idx);
            self.next_player = match self.next_player {
                Player::Black => Player::White,
                Player::White => Player::Black,
            };
        }
    }

    pub fn redo(&mut self) {
        if let Some(next_idx) = self.future_history.pop() {
            self.grid[next_idx] = Some(self.next_player);
            self.history.push(next_idx);
            self.next_player = match self.next_player {
                Player::Black => Player::White,
                Player::White => Player::Black,
            };
        }
    }

    pub fn undo_all(&mut self) {
        while !self.history.is_empty() {
            self.undo();
        }
    }

    pub fn redo_all(&mut self) {
        while !self.future_history.is_empty() {
            self.redo();
        }
    }
}