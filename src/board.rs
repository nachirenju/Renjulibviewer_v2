// 連珠盤の論理的な状態管理、石の配置、および履歴管理を行う
pub const SIZE: usize = 15;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Player {
    Black,
    White,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_toggle_replace_and_sequence_navigation() {
        let mut board = Board::new();
        board.place_stone(1, 1);
        board.begin_free_placement();
        assert!(board.history.is_empty());
        assert_eq!(board.setup_grid[16], Some(Player::Black));
        board.toggle_setup_stone(1, 1, Player::Black);
        assert_eq!(board.grid[16], None);
        board.toggle_setup_stone(2, 2, Player::White);
        board.toggle_setup_stone(2, 2, Player::Black);
        assert_eq!(board.grid[32], Some(Player::Black));
        board.toggle_setup_stone(3, 3, Player::White);
        board.toggle_setup_stone(3, 3, Player::White);
        assert_eq!(board.grid[48], None);
        board.begin_sequence(Player::White);
        assert!(!board.place_stone(2, 2));
        assert!(board.place_stone(4, 4));
        assert!(board.place_stone(5, 5));
        assert_eq!(board.history, vec![64, 80]);
        assert_eq!(board.grid[64], Some(Player::White));
        assert_eq!(board.grid[80], Some(Player::Black));
        assert_eq!(board.grid_at_history(&board.history), board.grid);
        assert_eq!(board.grid_at_history(&[]), board.setup_grid);
        board.undo_all();
        assert_eq!(board.grid, board.setup_grid);
        assert_eq!(board.next_player, Player::White);
        board.redo_all();
        assert_eq!(board.grid_at_history(&board.history), board.grid);
        board.begin_free_placement();
        assert!(board.history.is_empty());
        assert!(board.future_history.is_empty());
        board.begin_sequence(Player::Black);
        assert!(board.place_stone(6, 6));
        assert_eq!(board.history.len(), 1);
        assert_eq!(board.grid[96], Some(Player::Black));
    }
}

pub struct Board {
    pub grid: [Option<Player>; SIZE * SIZE],
    pub setup_grid: [Option<Player>; SIZE * SIZE],
    pub start_player: Player,
    pub custom_position: bool,
    pub next_player: Player,
    pub history: Vec<usize>,        // 履歴を追加
    pub future_history: Vec<usize>, // 取り消した手の履歴
}

impl Board {
    pub fn new() -> Self {
        Self {
            grid: [None; SIZE * SIZE],
            setup_grid: [None; SIZE * SIZE],
            start_player: Player::Black,
            custom_position: false,
            next_player: Player::Black,
            history: Vec::new(),
            future_history: Vec::new(),
        }
    }

    pub fn begin_free_placement(&mut self) {
        self.setup_grid = self.grid;
        self.history.clear();
        self.future_history.clear();
        self.custom_position = true;
    }

    pub fn toggle_setup_stone(&mut self, x: usize, y: usize, player: Player) {
        if x >= SIZE || y >= SIZE {
            return;
        }
        let idx = y * SIZE + x;
        self.grid[idx] = if self.grid[idx] == Some(player) {
            None
        } else {
            Some(player)
        };
        self.setup_grid = self.grid;
    }

    pub fn begin_sequence(&mut self, player: Player) {
        self.start_player = player;
        self.next_player = player;
    }

    pub fn grid_at_history(&self, history: &[usize]) -> [Option<Player>; SIZE * SIZE] {
        let mut grid = self.setup_grid;
        let mut player = self.start_player;
        for &idx in history {
            grid[idx] = Some(player);
            player = match player {
                Player::Black => Player::White,
                Player::White => Player::Black,
            };
        }
        grid
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
