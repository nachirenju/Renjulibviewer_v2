use super::blackforbiddenmove::{self, BlackForbiddenKind as Forbidden};
use super::model::{BOARD_CELLS, BOARD_SIZE};
use std::{
    cmp::Reverse,
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use web_time::Instant;

pub const EMPTY: u8 = 0;
pub const BLACK: u8 = 1;
pub const WHITE: u8 = 2;
const DIRS: [(isize, isize); 4] = [(1, 0), (0, 1), (1, 1), (1, -1)];

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Move {
    pub x: usize,
    pub y: usize,
    pub color: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleSnapshot {
    pub forbidden: Option<&'static str>,
    pub wins: bool,
    pub makes_four: bool,
    pub immediate_win: Option<usize>,
    pub defenses: Vec<usize>,
    pub attacks: Vec<usize>,
    pub straight_four: bool,
    pub four_count: usize,
}

#[derive(Debug)]
pub enum SolveOutcome {
    Found(Vec<Move>),
    NotFound,
    Timeout,
    Cancelled,
}

enum Search {
    Found(Vec<Move>),
    NotFound,
    Timeout,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TtKey {
    hash: u64,
    turn: u8,
    opponent_wins: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchMode {
    Fast,
    Shortest,
    Long,
}

#[derive(Clone, Copy)]
struct BitBoard {
    black: [u64; 4],
    white: [u64; 4],
}

impl BitBoard {
    fn from_cells(cells: &[u8; BOARD_CELLS]) -> Self {
        let mut board = Self {
            black: [0; 4],
            white: [0; 4],
        };
        for (idx, &stone) in cells.iter().enumerate() {
            if stone != EMPTY {
                board.set(idx, stone);
            }
        }
        board
    }

    #[inline]
    fn get(&self, idx: usize) -> u8 {
        let word = idx >> 6;
        let mask = 1u64 << (idx & 63);
        if self.black[word] & mask != 0 {
            BLACK
        } else if self.white[word] & mask != 0 {
            WHITE
        } else {
            EMPTY
        }
    }

    #[inline]
    fn set(&mut self, idx: usize, color: u8) {
        debug_assert!(color == BLACK || color == WHITE);
        let word = idx >> 6;
        let mask = 1u64 << (idx & 63);
        self.black[word] &= !mask;
        self.white[word] &= !mask;
        if color == BLACK {
            self.black[word] |= mask;
        } else {
            self.white[word] |= mask;
        }
    }

    #[inline]
    fn clear(&mut self, idx: usize) {
        let word = idx >> 6;
        let mask = !(1u64 << (idx & 63));
        self.black[word] &= mask;
        self.white[word] &= mask;
    }

    #[inline]
    fn is_empty(&self, idx: usize) -> bool {
        let word = idx >> 6;
        let mask = 1u64 << (idx & 63);
        (self.black[word] | self.white[word]) & mask == 0
    }

    fn to_cells(self) -> [u8; BOARD_CELLS] {
        let mut cells = [EMPTY; BOARD_CELLS];
        for (idx, cell) in cells.iter_mut().enumerate() {
            *cell = self.get(idx);
        }
        cells
    }
}

pub struct Solver {
    board: BitBoard,
    hash: u64,
    zobrist: [[u64; 3]; BOARD_CELLS],
    bounds: Option<(usize, usize, usize, usize)>,
    bounds_stack: Vec<Option<(usize, usize, usize, usize)>>,
    tt: HashMap<TtKey, usize>,
    legacy_tt: HashMap<u64, usize>,
    pv: HashMap<TtKey, usize>,
    mode: SearchMode,
    started: Instant,
    time_limit: Duration,
    cancel: Arc<AtomicBool>,
}

impl Solver {
    pub fn inspect_move(
        board: [u8; BOARD_CELLS],
        idx: usize,
        color: u8,
    ) -> Result<RuleSnapshot, String> {
        if idx >= BOARD_CELLS || (color != BLACK && color != WHITE) {
            return Err("invalid move".to_owned());
        }
        if board[idx] != EMPTY {
            return Err("occupied".to_owned());
        }
        let mut solver = Self::new(board, Duration::from_secs(1), SearchMode::Fast);
        let (x, y) = xy(idx);
        solver.board.set(idx, color);
        let wins = solver.check_win(x, y, color);
        let forbidden = if color == BLACK && !wins {
            solver.forbidden(x, y, true).map(Forbidden::as_str)
        } else {
            None
        };
        let makes_four = solver.makes_four_or_five(x, y, color);
        let defenses = solver.vcf_defenses(color, idx);
        let immediate_win = solver.winning_moves(other(color)).into_iter().next();
        let attacks = solver.vcf_attacks(color, &[]);
        let straight_four = solver.valid_straight_four(x, y, color);
        let mut four_count = 0;
        for (dx, dy) in DIRS {
            four_count += solver.four_patterns(x, y, dx, dy, color);
        }
        Ok(RuleSnapshot {
            forbidden,
            wins,
            makes_four,
            immediate_win,
            defenses,
            attacks,
            straight_four,
            four_count,
        })
    }

    pub fn immediate_win(board: [u8; BOARD_CELLS], color: u8) -> Option<usize> {
        let mut solver = Self::new(board, Duration::from_secs(1), SearchMode::Fast);
        solver.winning_moves(color).into_iter().next()
    }

    pub fn vcf_defenses_for(board: [u8; BOARD_CELLS], attacker: u8, last: usize) -> Vec<usize> {
        Self::new(board, Duration::from_secs(1), SearchMode::Fast).vcf_defenses(attacker, last)
    }

    pub fn vcf_attacks_for(board: [u8; BOARD_CELLS], color: u8) -> Vec<usize> {
        Self::new(board, Duration::from_secs(1), SearchMode::Fast).vcf_attacks(color, &[])
    }

    pub fn is_black_forbidden(board: [u8; BOARD_CELLS], idx: usize) -> Option<&'static str> {
        blackforbiddenmove::is_forbidden(board, idx).map(Forbidden::as_str)
    }

    pub fn solve(
        board: [u8; BOARD_CELLS],
        turn: u8,
        max_depth: usize,
        time_limit: Duration,
        mode: SearchMode,
    ) -> SolveOutcome {
        Self::solve_with_cancel(
            board,
            turn,
            max_depth,
            time_limit,
            Arc::new(AtomicBool::new(false)),
            mode,
        )
    }

    pub fn solve_with_cancel(
        board: [u8; BOARD_CELLS],
        turn: u8,
        max_depth: usize,
        time_limit: Duration,
        cancel: Arc<AtomicBool>,
        mode: SearchMode,
    ) -> SolveOutcome {
        let mut solver = Self::new_with_cancel(board, time_limit, cancel, mode);
        let empty_cells = BOARD_CELLS - board.iter().filter(|&&stone| stone != EMPTY).count();
        let effective_depth = max_depth.min(empty_cells).max(1);
        let depths = match mode {
            SearchMode::Fast => depth_schedule(effective_depth),
            SearchMode::Shortest => linear_depth_schedule(effective_depth),
            SearchMode::Long => depth_schedule(effective_depth),
        };
        for depth in depths {
            solver.tt.clear();
            solver.legacy_tt.clear();
            match solver.solve_at(turn, 0, depth) {
                Search::Found(path) => return SolveOutcome::Found(path),
                Search::Timeout => return SolveOutcome::Timeout,
                Search::Cancelled => return SolveOutcome::Cancelled,
                Search::NotFound => {}
            }
        }
        SolveOutcome::NotFound
    }

    pub fn solve_shortest(
        board: [u8; BOARD_CELLS],
        turn: u8,
        max_depth: usize,
        time_limit: Duration,
    ) -> SolveOutcome {
        Self::solve_shortest_with_cancel(
            board,
            turn,
            max_depth,
            time_limit,
            Arc::new(AtomicBool::new(false)),
        )
    }

    pub fn solve_shortest_with_cancel(
        board: [u8; BOARD_CELLS],
        turn: u8,
        max_depth: usize,
        time_limit: Duration,
        cancel: Arc<AtomicBool>,
    ) -> SolveOutcome {
        let mut solver = Self::new_with_cancel(board, time_limit, cancel, SearchMode::Shortest);
        let empty_cells = BOARD_CELLS - board.iter().filter(|&&stone| stone != EMPTY).count();
        let effective_depth = max_depth.min(empty_cells).max(1);
        for depth in linear_depth_schedule(effective_depth) {
            solver.tt.clear();
            match solver.solve_at(turn, 0, depth) {
                Search::Found(path) => return SolveOutcome::Found(path),
                Search::Timeout => return SolveOutcome::Timeout,
                Search::Cancelled => return SolveOutcome::Cancelled,
                Search::NotFound => {}
            }
        }
        SolveOutcome::NotFound
    }

    pub fn black_forbidden_points(board: [u8; BOARD_CELLS]) -> Vec<usize> {
        if side_to_move(&board) != Ok(BLACK) {
            return Vec::new();
        }
        Self::black_forbidden_points_unchecked(board)
    }

    pub fn black_forbidden_points_unchecked(board: [u8; BOARD_CELLS]) -> Vec<usize> {
        blackforbiddenmove::forbidden_points(board)
    }

    fn new(board: [u8; BOARD_CELLS], time_limit: Duration, mode: SearchMode) -> Self {
        Self::new_with_cancel(board, time_limit, Arc::new(AtomicBool::new(false)), mode)
    }

    fn new_with_cancel(
        board: [u8; BOARD_CELLS],
        time_limit: Duration,
        cancel: Arc<AtomicBool>,
        mode: SearchMode,
    ) -> Self {
        let mut zobrist = [[0; 3]; BOARD_CELLS];
        let mut seed = 0x8f3c_7a21_d5e6_b409;
        for cell in &mut zobrist {
            for value in cell.iter_mut().skip(1) {
                seed = splitmix64(seed);
                *value = seed;
            }
        }
        let mut hash = 0;
        let mut bounds = None;
        for (idx, &stone) in board.iter().enumerate() {
            if stone != EMPTY {
                hash ^= zobrist[idx][stone as usize];
                let (x, y) = xy(idx);
                bounds = Some(expand_bounds(bounds, x, y));
            }
        }
        Self {
            board: BitBoard::from_cells(&board),
            hash,
            zobrist,
            bounds,
            bounds_stack: Vec::with_capacity(128),
            tt: HashMap::new(),
            legacy_tt: HashMap::new(),
            pv: HashMap::new(),
            mode,
            started: Instant::now(),
            time_limit,
            cancel,
        }
    }

    fn solve_at(&mut self, turn: u8, depth: usize, max_depth: usize) -> Search {
        if self.timed_out() {
            return Search::Timeout;
        }
        if self.cancelled() {
            return Search::Cancelled;
        }
        if self.mode == SearchMode::Long {
            return self.solve_at_legacy(turn, depth, max_depth);
        }
        if self.mode == SearchMode::Shortest && depth + 1 > max_depth {
            return Search::NotFound;
        }
        let rest = max_depth.saturating_sub(depth);
        let opponent_wins = self.winning_moves(other(turn));
        let key = self.tt_key(turn, &opponent_wins);
        if self.tt.get(&key).is_some_and(|&cached| cached >= rest) {
            return Search::NotFound;
        }
        let mut attacks = self.vcf_attacks(turn, &opponent_wins);
        self.prioritize_pv(&mut attacks, key);
        if attacks.is_empty() {
            self.tt.insert(key, rest);
            return Search::NotFound;
        }
        for attack in attacks {
            match self.verify_move(attack, turn, depth, max_depth) {
                Search::Found(mut path) => {
                    self.pv.insert(key, attack);
                    path.insert(
                        0,
                        Move {
                            x: attack % BOARD_SIZE,
                            y: attack / BOARD_SIZE,
                            color: turn,
                        },
                    );
                    return Search::Found(path);
                }
                Search::Timeout => return Search::Timeout,
                Search::Cancelled => return Search::Cancelled,
                Search::NotFound => {}
            }
        }
        self.tt.insert(key, rest);
        Search::NotFound
    }

    fn solve_at_legacy(&mut self, turn: u8, depth: usize, max_depth: usize) -> Search {
        if self.timed_out() {
            return Search::Timeout;
        }
        if self.cancelled() {
            return Search::Cancelled;
        }
        let rest = max_depth.saturating_sub(depth);
        if self
            .legacy_tt
            .get(&self.hash)
            .is_some_and(|&cached| cached >= rest)
        {
            return Search::NotFound;
        }
        let opponent_wins = self.winning_moves(other(turn));
        let attacks = self.vcf_attacks_legacy(turn, &opponent_wins);
        if attacks.is_empty() {
            self.legacy_tt.insert(self.hash, rest);
            return Search::NotFound;
        }
        for attack in attacks {
            match self.verify_move_legacy(attack, turn, depth, max_depth) {
                Search::Found(mut path) => {
                    path.insert(
                        0,
                        Move {
                            x: attack % BOARD_SIZE,
                            y: attack / BOARD_SIZE,
                            color: turn,
                        },
                    );
                    return Search::Found(path);
                }
                Search::Timeout => return Search::Timeout,
                Search::Cancelled => return Search::Cancelled,
                Search::NotFound => {}
            }
        }
        self.legacy_tt.insert(self.hash, rest);
        Search::NotFound
    }

    fn verify_move(&mut self, attack: usize, turn: u8, depth: usize, max_depth: usize) -> Search {
        if self.timed_out() {
            return Search::Timeout;
        }
        if self.cancelled() {
            return Search::Cancelled;
        }
        let (ax, ay) = xy(attack);
        self.put(attack, turn);
        if self.check_win(ax, ay, turn) || self.valid_straight_four(ax, ay, turn) {
            self.remove(attack, turn);
            return Search::Found(Vec::new());
        }

        let opponent = other(turn);
        let mut defenses = self.vcf_defenses(turn, attack);
        if defenses.is_empty() {
            self.remove(attack, turn);
            return Search::Found(Vec::new());
        }
        if opponent == BLACK {
            defenses.retain(|&idx| {
                let (x, y) = xy(idx);
                self.board.set(idx, BLACK);
                let valid = self.forbidden(x, y, true).is_none();
                self.board.clear(idx);
                valid
            });
            if defenses.is_empty() {
                self.remove(attack, turn);
                return Search::Found(Vec::new());
            }
        }
        self.order_defenses(&mut defenses, opponent, turn);

        let mut longest = Vec::new();
        for defense in defenses {
            let (dx, dy) = xy(defense);
            self.put(defense, opponent);
            let result = if self.check_win(dx, dy, opponent) {
                Search::NotFound
            } else if self.mode == SearchMode::Shortest && depth + 2 > max_depth {
                Search::NotFound
            } else {
                match self.solve_at(turn, depth + 2, max_depth) {
                    Search::Found(mut path) => {
                        path.insert(
                            0,
                            Move {
                                x: dx,
                                y: dy,
                                color: opponent,
                            },
                        );
                        Search::Found(path)
                    }
                    other => other,
                }
            };
            self.remove(defense, opponent);

            match result {
                Search::Found(path) => {
                    if path.len() > longest.len() {
                        longest = path;
                    }
                }
                Search::Timeout => {
                    self.remove(attack, turn);
                    return Search::Timeout;
                }
                Search::Cancelled => {
                    self.remove(attack, turn);
                    return Search::Cancelled;
                }
                Search::NotFound => {
                    self.remove(attack, turn);
                    return Search::NotFound;
                }
            }
        }
        self.remove(attack, turn);
        Search::Found(longest)
    }

    fn verify_move_legacy(
        &mut self,
        attack: usize,
        turn: u8,
        depth: usize,
        max_depth: usize,
    ) -> Search {
        if self.timed_out() {
            return Search::Timeout;
        }
        if self.cancelled() {
            return Search::Cancelled;
        }
        let (ax, ay) = xy(attack);
        self.put(attack, turn);
        if self.check_win(ax, ay, turn) || self.valid_straight_four(ax, ay, turn) {
            self.remove(attack, turn);
            return Search::Found(Vec::new());
        }

        let opponent = other(turn);
        let mut defenses = self.vcf_defenses(turn, attack);
        if defenses.is_empty() {
            self.remove(attack, turn);
            return Search::Found(Vec::new());
        }
        if opponent == BLACK {
            defenses.retain(|&idx| {
                let (x, y) = xy(idx);
                self.board.set(idx, BLACK);
                let valid = self.forbidden(x, y, true).is_none();
                self.board.clear(idx);
                valid
            });
            if defenses.is_empty() {
                self.remove(attack, turn);
                return Search::Found(Vec::new());
            }
        }

        let mut longest = Vec::new();
        for defense in defenses {
            let (dx, dy) = xy(defense);
            self.put(defense, opponent);
            let result = if self.check_win(dx, dy, opponent) {
                Search::NotFound
            } else {
                match self.solve_at_legacy(turn, depth + 2, max_depth) {
                    Search::Found(mut path) => {
                        path.insert(
                            0,
                            Move {
                                x: dx,
                                y: dy,
                                color: opponent,
                            },
                        );
                        Search::Found(path)
                    }
                    other => other,
                }
            };
            self.remove(defense, opponent);

            match result {
                Search::Found(path) => {
                    if path.len() > longest.len() {
                        longest = path;
                    }
                }
                Search::Timeout => {
                    self.remove(attack, turn);
                    return Search::Timeout;
                }
                Search::Cancelled => {
                    self.remove(attack, turn);
                    return Search::Cancelled;
                }
                Search::NotFound => {
                    self.remove(attack, turn);
                    return Search::NotFound;
                }
            }
        }
        self.remove(attack, turn);
        Search::Found(longest)
    }

    fn vcf_attacks(&mut self, turn: u8, opponent_wins: &[usize]) -> Vec<usize> {
        let (min_x, max_x, min_y, max_y) = self.active_range();
        let mut result = Vec::new();
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let idx = y * BOARD_SIZE + x;
                if !self.board.is_empty(idx) {
                    continue;
                }
                self.board.set(idx, turn);
                let wins_now = self.check_win(x, y, turn);
                let stops_opponent = opponent_wins.is_empty()
                    || (opponent_wins.len() == 1 && opponent_wins[0] == idx);
                let candidate = if wins_now {
                    true
                } else if !stops_opponent {
                    false
                } else if turn == BLACK {
                    self.makes_four_or_five(x, y, turn) && self.forbidden(x, y, true).is_none()
                } else {
                    self.makes_four_or_five(x, y, turn)
                };
                if candidate {
                    result.push((idx, self.priority(x, y, turn)));
                }
                self.board.clear(idx);
            }
        }
        result.sort_by_key(|item| Reverse(item.1));
        result.into_iter().map(|(idx, _)| idx).collect()
    }

    fn vcf_attacks_legacy(&mut self, turn: u8, opponent_wins: &[usize]) -> Vec<usize> {
        let (min_x, max_x, min_y, max_y) = self.active_range();
        let mut result = Vec::new();
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let idx = y * BOARD_SIZE + x;
                if !self.board.is_empty(idx) {
                    continue;
                }
                self.board.set(idx, turn);
                let wins_now = self.check_win(x, y, turn);
                let stops_opponent = opponent_wins.is_empty()
                    || (opponent_wins.len() == 1 && opponent_wins[0] == idx);
                let candidate = if wins_now {
                    true
                } else if !stops_opponent {
                    false
                } else if turn == BLACK {
                    self.makes_four_or_five(x, y, turn) && self.forbidden(x, y, true).is_none()
                } else {
                    self.makes_four_or_five(x, y, turn)
                };
                self.board.clear(idx);
                if candidate {
                    result.push((idx, self.priority(x, y, turn)));
                }
            }
        }
        result.sort_by_key(|item| Reverse(item.1));
        result.into_iter().map(|(idx, _)| idx).collect()
    }

    fn prioritize_pv(&self, attacks: &mut [usize], key: TtKey) {
        let Some(&pv_move) = self.pv.get(&key) else {
            return;
        };
        if let Some(index) = attacks.iter().position(|&attack| attack == pv_move) {
            attacks.swap(0, index);
        }
    }

    fn order_defenses(&mut self, defenses: &mut [usize], defender: u8, attacker: u8) {
        let mut scored = defenses
            .iter()
            .copied()
            .map(|idx| (idx, self.defense_refutation_score(idx, defender, attacker)))
            .collect::<Vec<_>>();
        scored.sort_by_key(|&(_, score)| score);
        for (target, (idx, _)) in defenses.iter_mut().zip(scored) {
            *target = idx;
        }
    }

    fn defense_refutation_score(&mut self, idx: usize, defender: u8, attacker: u8) -> i32 {
        let (x, y) = xy(idx);
        self.board.set(idx, defender);
        let score = if self.check_win(x, y, defender) {
            -1_000_000
        } else {
            let attacks = self.vcf_attacks(attacker, &[]);
            if attacks.is_empty() {
                -500_000
            } else {
                (attacks.len() as i32) * 10_000 - self.priority(x, y, defender)
            }
        };
        self.board.clear(idx);
        score
    }

    fn vcf_defenses(&self, attacker: u8, last: usize) -> Vec<usize> {
        let (lx, ly) = xy(last);
        let mut seen = [false; BOARD_CELLS];
        let mut defenses = Vec::new();
        for (dx, dy) in DIRS {
            for start in -4..=0 {
                let mut stones = 0;
                let mut empties = 0;
                let mut empty = 0;
                let mut blocked = false;
                for i in 0..5 {
                    let x = lx as isize + (start + i) * dx;
                    let y = ly as isize + (start + i) * dy;
                    let Some(idx) = index(x, y) else {
                        blocked = true;
                        break;
                    };
                    match self.board.get(idx) {
                        value if value == attacker => stones += 1,
                        EMPTY => {
                            empties += 1;
                            empty = idx;
                        }
                        _ => {
                            blocked = true;
                            break;
                        }
                    }
                }
                if !blocked && stones == 4 && empties == 1 && !seen[empty] {
                    seen[empty] = true;
                    defenses.push(empty);
                }
            }
        }
        defenses
    }

    fn forbidden(&mut self, x: usize, y: usize, check_three: bool) -> Option<Forbidden> {
        let idx = y * BOARD_SIZE + x;
        blackforbiddenmove::placed_forbidden(self.board.to_cells(), idx, check_three)
    }

    fn four_patterns(&mut self, x: usize, y: usize, dx: isize, dy: isize, color: u8) -> usize {
        let mut spots = Vec::new();
        for start in -4..=0 {
            let mut stones = 0;
            let mut empties = 0;
            let mut gap = None;
            let mut blocked = false;
            for k in 0..5 {
                let pos = start + k;
                if pos == 0 {
                    stones += 1;
                    continue;
                }
                let Some(idx) = index(x as isize + pos * dx, y as isize + pos * dy) else {
                    blocked = true;
                    break;
                };
                match self.board.get(idx) {
                    value if value == color => stones += 1,
                    EMPTY => {
                        empties += 1;
                        gap = Some(idx);
                    }
                    _ => {
                        blocked = true;
                        break;
                    }
                }
            }
            if !blocked && stones == 4 && empties == 1 {
                let gap = gap.unwrap();
                if spots.contains(&gap) {
                    continue;
                }
                if color == BLACK {
                    let (gx, gy) = xy(gap);
                    self.board.set(gap, BLACK);
                    let count = self.line_count(gx, gy, dx, dy, BLACK);
                    self.board.clear(gap);
                    if count > 5 {
                        continue;
                    }
                }
                spots.push(gap);
            }
        }
        spots.len()
    }

    fn valid_straight_four(&mut self, x: usize, y: usize, color: u8) -> bool {
        for (dx, dy) in DIRS {
            for start in -4..=0 {
                if !self.is_value(x, y, dx, dy, start, EMPTY)
                    || !self.is_value(x, y, dx, dy, start + 5, EMPTY)
                    || (1..=4).any(|k| !self.is_value(x, y, dx, dy, start + k, color))
                {
                    continue;
                }
                if self.can_win_at(x, y, dx, dy, start, color)
                    && self.can_win_at(x, y, dx, dy, start + 5, color)
                {
                    return true;
                }
            }
        }
        false
    }

    fn makes_four_or_five(&mut self, x: usize, y: usize, color: u8) -> bool {
        for (dx, dy) in DIRS {
            let count = self.line_count(x, y, dx, dy, color);
            if (color == BLACK && count == 5) || (color == WHITE && count >= 5) {
                return true;
            }
            if color == BLACK && count >= 6 {
                continue;
            }
            if self.four_patterns(x, y, dx, dy, color) > 0 {
                return true;
            }
        }
        false
    }

    fn winning_moves(&mut self, turn: u8) -> Vec<usize> {
        let mut wins = Vec::new();
        let (min_x, max_x, min_y, max_y) = self.active_range();
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let idx = y * BOARD_SIZE + x;
                if !self.board.is_empty(idx) {
                    continue;
                }
                self.board.set(idx, turn);
                if self.check_win(x, y, turn) {
                    wins.push(idx);
                }
                self.board.clear(idx);
            }
        }
        wins
    }

    fn check_win(&self, x: usize, y: usize, color: u8) -> bool {
        DIRS.into_iter().any(|(dx, dy)| {
            let count = self.line_count(x, y, dx, dy, color);
            if color == BLACK {
                count == 5
            } else {
                count >= 5
            }
        })
    }

    fn line_count(&self, x: usize, y: usize, dx: isize, dy: isize, color: u8) -> usize {
        let mut count = 1;
        for sign in [-1, 1] {
            let mut step = 1;
            while let Some(idx) =
                index(x as isize + dx * step * sign, y as isize + dy * step * sign)
            {
                if self.board.get(idx) != color {
                    break;
                }
                count += 1;
                step += 1;
            }
        }
        count
    }

    fn is_value(&self, x: usize, y: usize, dx: isize, dy: isize, k: isize, expected: u8) -> bool {
        if k == 0 {
            return true;
        }
        index(x as isize + k * dx, y as isize + k * dy)
            .is_some_and(|idx| self.board.get(idx) == expected)
    }

    fn can_win_at(
        &mut self,
        x: usize,
        y: usize,
        dx: isize,
        dy: isize,
        k: isize,
        color: u8,
    ) -> bool {
        let Some(idx) = index(x as isize + k * dx, y as isize + k * dy) else {
            return false;
        };
        if !self.board.is_empty(idx) {
            return false;
        }
        let (tx, ty) = xy(idx);
        self.board.set(idx, color);
        let wins = self.check_win(tx, ty, color);
        self.board.clear(idx);
        wins
    }

    fn active_range(&self) -> (usize, usize, usize, usize) {
        let Some((min_x, max_x, min_y, max_y)) = self.bounds else {
            return (4, 10, 4, 10);
        };
        (
            min_x.saturating_sub(2),
            (max_x + 2).min(BOARD_SIZE - 1),
            min_y.saturating_sub(2),
            (max_y + 2).min(BOARD_SIZE - 1),
        )
    }

    fn priority(&self, x: usize, y: usize, color: u8) -> i32 {
        let mut score = 0;
        for oy in -1..=1 {
            for ox in -1..=1 {
                if ox == 0 && oy == 0 {
                    continue;
                }
                if let Some(idx) = index(x as isize + ox, y as isize + oy) {
                    if !self.board.is_empty(idx) {
                        score += 10;
                    }
                    if self.board.get(idx) == color {
                        score += 5;
                    }
                }
            }
        }
        score - (x.abs_diff(7) + y.abs_diff(7)) as i32
    }

    fn put(&mut self, idx: usize, color: u8) {
        debug_assert!(self.board.is_empty(idx));
        self.bounds_stack.push(self.bounds);
        let (x, y) = xy(idx);
        self.bounds = Some(expand_bounds(self.bounds, x, y));
        self.board.set(idx, color);
        self.hash ^= self.zobrist[idx][color as usize];
    }

    fn remove(&mut self, idx: usize, color: u8) {
        self.hash ^= self.zobrist[idx][color as usize];
        debug_assert_eq!(self.board.get(idx), color);
        self.board.clear(idx);
        self.bounds = self
            .bounds_stack
            .pop()
            .expect("put/remove bounds stack must stay balanced");
    }

    fn timed_out(&self) -> bool {
        self.started.elapsed() >= self.time_limit
    }

    #[inline]
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn tt_key(&self, turn: u8, opponent_wins: &[usize]) -> TtKey {
        TtKey {
            hash: self.hash,
            turn,
            opponent_wins: opponent_wins_signature(opponent_wins),
        }
    }
}

fn opponent_wins_signature(opponent_wins: &[usize]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    opponent_wins.len().hash(&mut hasher);
    for &idx in opponent_wins {
        idx.hash(&mut hasher);
    }
    hasher.finish()
}

pub fn side_to_move(board: &[u8; BOARD_CELLS]) -> Result<u8, String> {
    let black = board.iter().filter(|&&stone| stone == BLACK).count();
    let white = board.iter().filter(|&&stone| stone == WHITE).count();
    match black.checked_sub(white) {
        Some(0) => Ok(BLACK),
        Some(1) => Ok(WHITE),
        _ => Err(format!("石数が不正です（黒 {black} / 白 {white}）")),
    }
}

fn other(color: u8) -> u8 {
    if color == BLACK { WHITE } else { BLACK }
}

fn xy(idx: usize) -> (usize, usize) {
    (idx % BOARD_SIZE, idx / BOARD_SIZE)
}

fn index(x: isize, y: isize) -> Option<usize> {
    if x >= 0 && x < BOARD_SIZE as isize && y >= 0 && y < BOARD_SIZE as isize {
        Some(y as usize * BOARD_SIZE + x as usize)
    } else {
        None
    }
}

fn expand_bounds(
    bounds: Option<(usize, usize, usize, usize)>,
    x: usize,
    y: usize,
) -> (usize, usize, usize, usize) {
    bounds.map_or((x, x, y, y), |(min_x, max_x, min_y, max_y)| {
        (min_x.min(x), max_x.max(x), min_y.min(y), max_y.max(y))
    })
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn linear_depth_schedule(max_depth: usize) -> Vec<usize> {
    let max_depth = if max_depth % 2 == 0 {
        max_depth.saturating_sub(1).max(1)
    } else {
        max_depth
    };
    let mut depths: Vec<usize> = (1..=max_depth).step_by(2).collect();
    if depths.last().copied() != Some(max_depth) {
        depths.push(max_depth);
    }
    depths
}

fn depth_schedule(max_depth: usize) -> Vec<usize> {
    let max_depth = if max_depth.is_multiple_of(2) {
        max_depth.saturating_sub(1).max(1)
    } else {
        max_depth
    };
    let mut depths: Vec<usize> = (1..=max_depth.min(21)).step_by(2).collect();
    let mut depth = 29;
    while depth < max_depth {
        depths.push(depth);
        depth += 8;
    }
    if depths.last().copied() != Some(max_depth) {
        depths.push(max_depth);
    }
    depths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitboard_set_replace_and_clear() {
        let cells = [0; BOARD_CELLS];
        let mut board = BitBoard::from_cells(&cells);
        for idx in [0, 63, 64, 127, 128, 191, 224] {
            board.set(idx, BLACK);
            assert_eq!(board.get(idx), BLACK);
            board.set(idx, WHITE);
            assert_eq!(board.get(idx), WHITE);
            board.clear(idx);
            assert!(board.is_empty(idx));
        }
    }

    #[test]
    fn long_depth_schedule_reaches_beyond_fifty_without_every_iteration() {
        let schedule = depth_schedule(61);
        assert_eq!(schedule.last(), Some(&61));
        assert!(schedule.contains(&53));
        assert!(!schedule.contains(&51));
        assert!(schedule.len() < 20);
    }

    #[test]
    fn cancelled_search_stops_before_work() {
        let cancel = Arc::new(AtomicBool::new(true));
        let result = Solver::solve_with_cancel(
            [0; BOARD_CELLS],
            BLACK,
            81,
            Duration::from_secs(10),
            cancel,
            SearchMode::Fast,
        );
        assert!(matches!(result, SolveOutcome::Cancelled));
    }

    #[test]
    fn cached_active_range_restores_after_remove() {
        let mut board = [0; BOARD_CELLS];
        board[7 * BOARD_SIZE + 7] = BLACK;
        let mut solver = Solver::new(board, Duration::from_secs(1), SearchMode::Fast);
        let original = solver.active_range();
        solver.put(14 * BOARD_SIZE + 14, WHITE);
        assert_eq!(solver.active_range(), (5, 14, 5, 14));
        solver.remove(14 * BOARD_SIZE + 14, WHITE);
        assert_eq!(solver.active_range(), original);
    }

    #[test]
    fn side_to_move_checks_counts() {
        let mut board = [0; BOARD_CELLS];
        assert_eq!(side_to_move(&board).unwrap(), BLACK);
        board[112] = BLACK;
        assert_eq!(side_to_move(&board).unwrap(), WHITE);
        board[113] = BLACK;
        assert!(side_to_move(&board).is_err());
    }

    #[test]
    fn finds_immediate_five() {
        let mut board = [0; BOARD_CELLS];
        for x in 4..8 {
            board[7 * BOARD_SIZE + x] = BLACK;
        }
        let result = Solver::solve(board, BLACK, 1, Duration::from_secs(1), SearchMode::Fast);
        assert!(matches!(result, SolveOutcome::Found(path) if path.len() == 1));
    }

    #[test]
    fn long_mode_keeps_fast_mode_candidate_order_for_basic_win() {
        let mut board = [0; BOARD_CELLS];
        for x in 4..8 {
            board[7 * BOARD_SIZE + x] = BLACK;
        }
        let fast = Solver::solve(board, BLACK, 9, Duration::from_secs(1), SearchMode::Fast);
        let long = Solver::solve(board, BLACK, 9, Duration::from_secs(1), SearchMode::Long);
        assert!(matches!(fast, SolveOutcome::Found(path) if path.len() == 1));
        assert!(matches!(long, SolveOutcome::Found(path) if path.len() == 1));
    }

    #[test]
    fn does_not_ignore_opponents_open_four() {
        let mut board = [0; BOARD_CELLS];
        for x in 3..7 {
            board[3 * BOARD_SIZE + x] = WHITE;
        }
        for x in 4..7 {
            board[10 * BOARD_SIZE + x] = BLACK;
        }
        board[12 * BOARD_SIZE + 12] = BLACK;

        let result = Solver::solve(board, BLACK, 3, Duration::from_secs(1), SearchMode::Fast);
        assert!(matches!(result, SolveOutcome::NotFound));
    }

    #[test]
    fn forbidden_overline_survives_bitboard_conversion() {
        let mut cells = [0; BOARD_CELLS];
        for x in 3..9 {
            cells[7 * BOARD_SIZE + x] = BLACK;
        }
        let mut solver = Solver::new(cells, Duration::from_secs(1), SearchMode::Fast);
        assert_eq!(solver.forbidden(8, 7, true), Some(Forbidden::Overline));
    }

    #[test]
    fn forbidden_double_four_survives_bitboard_conversion() {
        let mut cells = [0; BOARD_CELLS];
        for x in 4..=7 {
            cells[7 * BOARD_SIZE + x] = BLACK;
        }
        for y in 4..=6 {
            cells[y * BOARD_SIZE + 7] = BLACK;
        }
        let mut solver = Solver::new(cells, Duration::from_secs(1), SearchMode::Fast);
        assert_eq!(solver.forbidden(7, 7, true), Some(Forbidden::DoubleFour));
    }

    #[test]
    fn forbidden_double_three_survives_bitboard_conversion() {
        let mut cells = [0; BOARD_CELLS];
        for (x, y) in [(6, 7), (7, 7), (8, 7), (7, 6), (7, 8)] {
            cells[y * BOARD_SIZE + x] = BLACK;
        }
        let mut solver = Solver::new(cells, Duration::from_secs(1), SearchMode::Fast);
        assert_eq!(solver.forbidden(7, 7, true), Some(Forbidden::DoubleThree));
    }

    #[test]
    fn forbidden_points_include_double_three_but_not_exact_five() {
        let mut double_three = [0; BOARD_CELLS];
        for (x, y) in [(6, 7), (8, 7), (7, 6), (7, 8)] {
            double_three[y * BOARD_SIZE + x] = BLACK;
        }
        for (x, y) in [(0, 0), (1, 0), (2, 0), (3, 0)] {
            double_three[y * BOARD_SIZE + x] = WHITE;
        }
        assert!(Solver::black_forbidden_points(double_three).contains(&(7 * BOARD_SIZE + 7)));

        let mut exact_five = [0; BOARD_CELLS];
        for x in 3..7 {
            exact_five[7 * BOARD_SIZE + x] = BLACK;
        }
        exact_five[..4].fill(WHITE);
        assert!(!Solver::black_forbidden_points(exact_five).contains(&(7 * BOARD_SIZE + 7)));
    }

    #[test]
    fn false_double_three_ignores_three_whose_vital_point_is_forbidden() {
        let mut cells = [0; BOARD_CELLS];
        for (x, y) in [
            (6, 7),
            (8, 7),
            (7, 8),
            (7, 10),
            (6, 9),
            (8, 9),
            (6, 8),
            (8, 10),
        ] {
            cells[y * BOARD_SIZE + x] = BLACK;
        }

        let b = 7 * BOARD_SIZE + 7;
        let a = 9 * BOARD_SIZE + 7;

        let mut with_b = cells;
        with_b[b] = BLACK;
        assert_eq!(Solver::is_black_forbidden(with_b, a), Some("double-three"));
        assert_eq!(Solver::is_black_forbidden(cells, b), None);
    }

}
