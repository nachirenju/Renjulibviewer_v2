use super::{
    blackforbiddenmove::{self, BlackForbiddenKind as Forbidden},
    vcf_threat,
};
use super::model::{BOARD_CELLS, BOARD_SIZE};
use std::{
    cmp::Reverse,
    hash::{Hash, Hasher},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        OnceLock,
    },
    time::Duration,
};
#[cfg(target_os = "windows")]
use std::{
    collections::VecDeque,
    sync::Mutex,
};
use web_time::Instant;
use rustc_hash::FxHashMap;

// Public solve entry points live in their own search-family modules.  The
// board/rule primitives and shared caches remain here because both solvers
// mutate the same `Solver` state.
#[path = "VCF.rs"]
mod vcf_search;
#[path = "VCT.rs"]
mod vct_search;

pub const EMPTY: u8 = 0;
pub const BLACK: u8 = 1;
pub const WHITE: u8 = 2;
const DIRS: [(isize, isize); 4] = [(1, 0), (0, 1), (1, 1), (1, -1)];
const SEARCH_CACHE_SOFT_LIMIT: usize = 120_000;
const SEARCH_TT_SOFT_LIMIT: usize = 240_000;
#[cfg(target_os = "windows")]
const VCT_TRACE_LOG_LIMIT: usize = 500;

#[cfg(target_os = "windows")]
static VCT_TRACE_LOG: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();

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
    TacticalFound(Vec<Move>),
    NotFound,
    Incomplete,
    Timeout,
    Cancelled,
}

enum Search {
    Found(Vec<Move>),
    NotFound,
    Incomplete,
    Timeout,
    Cancelled,
    Backoff,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AttackKind {
    Five,
    StraightFour,
    Four,
}

#[derive(Clone, Debug)]
struct VcfAttack {
    mv: usize,
    kind: AttackKind,
    defenses: Vec<usize>,
    score: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TtKey {
    hash: u64,
    turn: u8,
    opponent_wins: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ForbiddenKey {
    black: [u64; 4],
    white: [u64; 4],
    idx: u8,
    check_three: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct BoardColorKey {
    black: [u64; 4],
    white: [u64; 4],
    turn: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct AttackCacheKey {
    black: [u64; 4],
    white: [u64; 4],
    turn: u8,
    opponent_wins: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchMode {
    Fast,
    FirstWin,
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
    tt: FxHashMap<TtKey, usize>,
    legacy_tt: FxHashMap<u64, usize>,
    pv: FxHashMap<TtKey, usize>,
    forbidden_cache: FxHashMap<ForbiddenKey, Option<Forbidden>>,
    winning_cache: FxHashMap<BoardColorKey, Vec<usize>>,
    attack_cache: FxHashMap<AttackCacheKey, Vec<VcfAttack>>,
    legacy_attack_cache: FxHashMap<AttackCacheKey, Vec<usize>>,
    /// Temporary deadline used by bounded VCF probes in Threat detection.
    local_deadline: Option<Instant>,
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
        let attacks = solver
            .vcf_attacks(color, &[])
            .into_iter()
            .map(|attack| attack.mv)
            .collect();
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

    pub fn is_legal_non_four_move(
        board: [u8; BOARD_CELLS],
        idx: usize,
        color: u8,
    ) -> Result<bool, String> {
        if idx >= BOARD_CELLS || (color != BLACK && color != WHITE) {
            return Err("invalid move".to_owned());
        }
        if board[idx] != EMPTY {
            return Err("occupied".to_owned());
        }

        let mut solver = Self::new(board, Duration::from_secs(1), SearchMode::Fast);
        let (x, y) = xy(idx);
        solver.board.set(idx, color);
        if solver.check_win(x, y, color) || solver.makes_four_or_five(x, y, color) {
            return Ok(false);
        }
        if color == BLACK && solver.forbidden(x, y, true).is_some() {
            return Ok(false);
        }

        Ok(true)
    }

    pub fn legal_non_straight_four_after(
        board: [u8; BOARD_CELLS],
        idx: usize,
        color: u8,
    ) -> Result<bool, String> {
        if idx >= BOARD_CELLS || (color != BLACK && color != WHITE) {
            return Err("invalid move".to_owned());
        }
        if board[idx] != EMPTY {
            return Err("occupied".to_owned());
        }

        Ok(Self::new(board, Duration::from_secs(1), SearchMode::Fast)
            .legal_non_straight_four_on_current_board(idx, color))
    }

    pub fn legal_straight_four_after(
        board: [u8; BOARD_CELLS],
        idx: usize,
        color: u8,
    ) -> Result<bool, String> {
        if idx >= BOARD_CELLS || (color != BLACK && color != WHITE) {
            return Err("invalid move".to_owned());
        }
        if board[idx] != EMPTY {
            return Err("occupied".to_owned());
        }

        Ok(Self::new(board, Duration::from_secs(1), SearchMode::Fast)
            .legal_straight_four_on_current_board(idx, color))
    }

    pub(crate) fn active_three_direction_mask_on_current_board(
        &mut self,
        idx: usize,
        color: u8,
    ) -> u8 {
        if idx >= BOARD_CELLS || !self.board.is_empty(idx) {
            return 0;
        }

        let x = idx % BOARD_SIZE;
        let y = idx / BOARD_SIZE;
        let mut mask = 0u8;
        let mut seen = [false; BOARD_CELLS];
        let mut points = Vec::new();

        for (dir_idx, (dx, dy)) in DIRS.iter().copied().enumerate() {
            for offset in -4..=4 {
                if offset == 0 {
                    continue;
                }
                let Some(next_idx) = index(x as isize + dx * offset, y as isize + dy * offset)
                else {
                    continue;
                };
                if seen[next_idx] {
                    continue;
                }
                seen[next_idx] = true;
                if !self.board.is_empty(next_idx) {
                    continue;
                }
                points.push((dir_idx, next_idx));
            }
        }

        let mut before_straight_four = [false; BOARD_CELLS];
        for &(_, next_idx) in &points {
            before_straight_four[next_idx] =
                self.legal_straight_four_on_current_board(next_idx, color);
        }

        self.put(idx, color);
        for (dir_idx, next_idx) in points {
            if self.board.is_empty(next_idx) {
                let straight_four = self.legal_straight_four_on_current_board(next_idx, color);
                if straight_four && !before_straight_four[next_idx] {
                    mask |= 1 << dir_idx;
                }
            }
        }
        self.remove(idx, color);
        mask
    }

    /// Returns whether an empty point is a legal non-four move on the current
    /// board.  The Threat finder uses this to exclude direct VCF moves before
    /// probing pass-VCF Threats.
    pub(crate) fn is_legal_non_four_on_current_board(&mut self, idx: usize, color: u8) -> bool {
        if idx >= BOARD_CELLS || !self.board.is_empty(idx) || (color != BLACK && color != WHITE) {
            return false;
        }

        let (x, y) = xy(idx);
        self.board.set(idx, color);
        let legal = !self.check_win(x, y, color)
            && !self.makes_four_or_five(x, y, color)
            && (color != BLACK || self.forbidden(x, y, true).is_none());
        self.board.clear(idx);
        legal
    }

    pub fn immediate_win(board: [u8; BOARD_CELLS], color: u8) -> Option<usize> {
        let mut solver = Self::new(board, Duration::from_secs(1), SearchMode::Fast);
        solver.winning_moves(color).into_iter().next()
    }

    pub fn vcf_defenses_for(board: [u8; BOARD_CELLS], attacker: u8, last: usize) -> Vec<usize> {
        Self::new(board, Duration::from_secs(1), SearchMode::Fast).vcf_defenses(attacker, last)
    }

    pub fn vcf_attacks_for(board: [u8; BOARD_CELLS], color: u8) -> Vec<usize> {
        Self::new(board, Duration::from_secs(1), SearchMode::Fast)
            .vcf_attacks(color, &[])
            .into_iter()
            .map(|attack| attack.mv)
            .collect()
    }

    pub fn find_vcf_threats_reusing_cache(
        board: [u8; BOARD_CELLS],
        turn: u8,
        max_depth: usize,
        time_limit: Duration,
        mode: SearchMode,
        cancel: Option<Arc<AtomicBool>>,
    ) -> vcf_threat::ThreatSearchOutcome {
        vcf_search::find_threats(board, turn, max_depth, time_limit, mode, cancel)
    }

    fn find_vcf_threats_engine(
        board: [u8; BOARD_CELLS],
        turn: u8,
        max_depth: usize,
        time_limit: Duration,
        mode: SearchMode,
        cancel: Option<Arc<AtomicBool>>,
    ) -> vcf_threat::ThreatSearchOutcome {
        let cancel_flag = cancel.unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        let mut solver = Self::new_with_cancel(board, time_limit, cancel_flag.clone(), mode);
        let started = Instant::now();
        let mut threats = Vec::new();
        let mut candidates = Vec::new();

        for idx in vcf_threat::ordered_threat_candidates(&board, turn) {
            if cancel_flag.load(Ordering::Relaxed) {
                return vcf_threat::ThreatSearchOutcome::Cancelled;
            }
            if time_limit.checked_sub(started.elapsed()).is_none() {
                return vcf_threat::ThreatSearchOutcome::Partial(threats);
            }
            // Reuse the solver that will immediately generate VCF attacks.
            // The old helper constructed a short-lived Solver for every
            // candidate and then repeated the same board/rule work here.
            if !solver.is_legal_non_four_on_current_board(idx, turn)
                || solver.active_three_direction_mask_on_current_board(idx, turn) != 0
            {
                continue;
            }

            solver.board.set(idx, turn);
            let attack_count = solver.vcf_attacks(turn, &[]).len();
            solver.board.clear(idx);
            if attack_count == 0 {
                continue;
            }

            let x = idx % BOARD_SIZE;
            let y = idx / BOARD_SIZE;
            let center_distance = x.abs_diff(BOARD_SIZE / 2) + y.abs_diff(BOARD_SIZE / 2);
            candidates.push((idx, attack_count, center_distance));
        }

        candidates.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.2.cmp(&b.2)));

        let mut found = [false; BOARD_CELLS];
        let quick_limit = vcf_threat_quick_candidate_time_limit(time_limit);
        let quick_probe_count = candidates.len().min(32);

        for (idx, _, _) in candidates.iter().copied().take(quick_probe_count) {
            if cancel_flag.load(Ordering::Relaxed) {
                return vcf_threat::ThreatSearchOutcome::Cancelled;
            }
            let Some(remaining) = time_limit.checked_sub(started.elapsed()) else {
                return vcf_threat::ThreatSearchOutcome::Partial(threats);
            };
            if remaining.is_zero() {
                return vcf_threat::ThreatSearchOutcome::Partial(threats);
            }

            if matches!(
                solver.probe_vcf_after_move(idx, turn, max_depth, Some(remaining.min(quick_limit))),
                Search::Found(_)
            ) {
                threats.push(Move {
                    x: idx % BOARD_SIZE,
                    y: idx / BOARD_SIZE,
                    color: turn,
                });
                found[idx] = true;
            }
        }

        let candidate_count = candidates.len();
        let mut incomplete = false;
        for (candidate_index, (idx, _, _)) in candidates.into_iter().enumerate() {
            if found[idx] {
                continue;
            }
            if cancel_flag.load(Ordering::Relaxed) {
                return vcf_threat::ThreatSearchOutcome::Cancelled;
            }
            let Some(remaining) = time_limit.checked_sub(started.elapsed()) else {
                return vcf_threat::ThreatSearchOutcome::Partial(threats);
            };
            if remaining.is_zero() {
                return vcf_threat::ThreatSearchOutcome::Partial(threats);
            }

            let candidates_left = candidate_count.saturating_sub(candidate_index).max(1);
            let candidate_limit =
                remaining.min(vcf_threat_candidate_time_limit(remaining, candidates_left));
            match solver.probe_vcf_after_move(idx, turn, max_depth, Some(candidate_limit)) {
                Search::Found(_) => threats.push(Move {
                    x: idx % BOARD_SIZE,
                    y: idx / BOARD_SIZE,
                    color: turn,
                }),
                Search::Timeout | Search::Incomplete | Search::Backoff => {
                    incomplete = true;
                }
                Search::Cancelled => return vcf_threat::ThreatSearchOutcome::Cancelled,
                Search::NotFound => {}
            }
        }

        if incomplete {
            vcf_threat::ThreatSearchOutcome::Partial(threats)
        } else {
            vcf_threat::ThreatSearchOutcome::Complete(threats)
        }
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
        vcf_search::solve(board, turn, max_depth, time_limit, cancel, mode)
    }

    pub fn solve_vct(
        board: [u8; BOARD_CELLS],
        turn: u8,
        max_depth: usize,
        time_limit: Duration,
        mode: SearchMode,
    ) -> SolveOutcome {
        Self::solve_vct_with_cancel(
            board,
            turn,
            max_depth,
            time_limit,
            Arc::new(AtomicBool::new(false)),
            mode,
        )
    }

    pub fn solve_vct_with_cancel(
        board: [u8; BOARD_CELLS],
        turn: u8,
        max_depth: usize,
        time_limit: Duration,
        cancel: Arc<AtomicBool>,
        mode: SearchMode,
    ) -> SolveOutcome {
        vct_search::solve(board, turn, max_depth, time_limit, cancel, mode)
    }

    pub fn solve_vct_with_progress<F>(
        board: [u8; BOARD_CELLS],
        turn: u8,
        max_depth: usize,
        time_limit: Duration,
        cancel: Arc<AtomicBool>,
        mode: SearchMode,
        progress: F,
    ) -> SolveOutcome
    where
        F: FnMut(Vec<Move>) + Send + 'static,
    {
        vct_search::solve_with_progress(board, turn, max_depth, time_limit, cancel, mode, progress)
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
        vcf_search::solve_shortest(board, turn, max_depth, time_limit, cancel)
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

    pub(crate) fn new(board: [u8; BOARD_CELLS], time_limit: Duration, mode: SearchMode) -> Self {
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
            tt: FxHashMap::default(),
            legacy_tt: FxHashMap::default(),
            pv: FxHashMap::default(),
            forbidden_cache: FxHashMap::default(),
            winning_cache: FxHashMap::default(),
            attack_cache: FxHashMap::default(),
            legacy_attack_cache: FxHashMap::default(),
            local_deadline: None,
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
        if self.local_deadline_expired() {
            return Search::Backoff;
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
            clear_if_over_limit(&mut self.tt, SEARCH_TT_SOFT_LIMIT);
            self.tt.insert(key, rest);
            return Search::NotFound;
        }
        for attack in attacks {
            let result = self.verify_move(&attack, turn, depth, max_depth);

            match result {
                Search::Found(mut path) => {
                    self.pv.insert(key, attack.mv);
                    path.insert(
                        0,
                        Move {
                            x: attack.mv % BOARD_SIZE,
                            y: attack.mv / BOARD_SIZE,
                            color: turn,
                        },
                    );
                    return Search::Found(path);
                }
                Search::Timeout => return Search::Timeout,
                Search::Cancelled => return Search::Cancelled,
                Search::Incomplete => return Search::Incomplete,
                Search::Backoff => return Search::Backoff,
                Search::NotFound => {}
            }
        }
        clear_if_over_limit(&mut self.tt, SEARCH_TT_SOFT_LIMIT);
        self.tt.insert(key, rest);
        Search::NotFound
    }


    fn probe_vcf_after_move(
        &mut self,
        idx: usize,
        attacker: u8,
        max_depth: usize,
        local_limit: Option<Duration>,
    ) -> Search {
        if idx >= BOARD_CELLS || !self.board.is_empty(idx) {
            return Search::NotFound;
        }

        let before_black = self.board.black;
        let before_white = self.board.white;
        let before_hash = self.hash;
        let before_bounds = self.bounds;
        let before_bounds_stack_len = self.bounds_stack.len();
        let previous_deadline = self.local_deadline;

        self.put(idx, attacker);
        if let Some(limit) = local_limit {
            self.local_deadline = Some(Instant::now() + limit);
        }
        let result = match self.solve_at(attacker, 0, max_depth) {
            Search::Backoff if local_limit.is_some() => Search::Incomplete,
            other => other,
        };
        self.local_deadline = previous_deadline;
        self.remove(idx, attacker);

        self.bounds_stack.truncate(before_bounds_stack_len);
        debug_assert_eq!(self.board.black, before_black);
        debug_assert_eq!(self.board.white, before_white);
        debug_assert_eq!(self.hash, before_hash);
        debug_assert_eq!(self.bounds, before_bounds);
        debug_assert_eq!(self.bounds_stack.len(), before_bounds_stack_len);

        result
    }








    fn solve_at_legacy(&mut self, turn: u8, depth: usize, max_depth: usize) -> Search {
        if self.timed_out() {
            return Search::Timeout;
        }
        if self.local_deadline_expired() {
            return Search::Backoff;
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
            clear_if_over_limit(&mut self.legacy_tt, SEARCH_TT_SOFT_LIMIT);
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
                Search::Incomplete => return Search::Incomplete,
                Search::Backoff => return Search::Backoff,
                Search::NotFound => {}
            }
        }
        clear_if_over_limit(&mut self.legacy_tt, SEARCH_TT_SOFT_LIMIT);
        self.legacy_tt.insert(self.hash, rest);
        Search::NotFound
    }

    fn verify_move(
        &mut self,
        attack: &VcfAttack,
        turn: u8,
        depth: usize,
        max_depth: usize,
    ) -> Search {
        if self.timed_out() {
            return Search::Timeout;
        }
        if self.local_deadline_expired() {
            return Search::Incomplete;
        }
        if self.cancelled() {
            return Search::Cancelled;
        }
        self.put(attack.mv, turn);
        if attack.kind != AttackKind::Four {
            self.remove(attack.mv, turn);
            return Search::Found(Vec::new());
        }

        let opponent = other(turn);
        let mut defenses = attack.defenses.clone();
        if defenses.is_empty() {
            self.remove(attack.mv, turn);
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
                self.remove(attack.mv, turn);
                return Search::Found(Vec::new());
            }
        }
        self.order_defenses(&mut defenses, opponent, attack.mv);

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
                    self.remove(attack.mv, turn);
                    return Search::Timeout;
                }
                Search::Cancelled => {
                    self.remove(attack.mv, turn);
                    return Search::Cancelled;
                }
                Search::Incomplete => {
                    self.remove(attack.mv, turn);
                    return Search::Incomplete;
                }
                Search::Backoff => {
                    self.remove(attack.mv, turn);
                    return Search::Backoff;
                }
                Search::NotFound => {
                    self.remove(attack.mv, turn);
                    return Search::NotFound;
                }
            }
        }
        self.remove(attack.mv, turn);
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
                Search::Incomplete => {
                    self.remove(attack, turn);
                    return Search::Incomplete;
                }
                Search::Backoff => {
                    self.remove(attack, turn);
                    return Search::Backoff;
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

    fn vcf_attacks(&mut self, turn: u8, opponent_wins: &[usize]) -> Vec<VcfAttack> {
        let key = AttackCacheKey {
            black: self.board.black,
            white: self.board.white,
            turn,
            opponent_wins: opponent_wins_signature(opponent_wins),
        };
        if let Some(cached) = self.attack_cache.get(&key) {
            return cached.clone();
        }

        let mut seen = [false; BOARD_CELLS];
        let mut result = Vec::new();
        for idx in self.line_attack_candidates(turn, true) {
            if seen[idx] || !self.board.is_empty(idx) {
                continue;
            }
            seen[idx] = true;
            if let Some(attack) = self.vcf_attack_after_move(idx, turn, opponent_wins) {
                result.push(attack);
            }
        }

        for idx in 0..BOARD_CELLS {
            if seen[idx] || !self.board.is_empty(idx) {
                continue;
            }
            seen[idx] = true;
            if let Some(attack) = self.vcf_attack_after_move(idx, turn, opponent_wins) {
                result.push(attack);
            }
        }

        result.sort_by_key(|attack| Reverse(attack.score));
        clear_if_over_limit(&mut self.attack_cache, SEARCH_CACHE_SOFT_LIMIT);
        self.attack_cache.insert(key, result.clone());
        result
    }

    fn vcf_attack_after_move(
        &mut self,
        idx: usize,
        turn: u8,
        opponent_wins: &[usize],
    ) -> Option<VcfAttack> {
        let (x, y) = xy(idx);
        self.board.set(idx, turn);
        let wins_now = self.check_win(x, y, turn);
        let stops_opponent =
            opponent_wins.is_empty() || (opponent_wins.len() == 1 && opponent_wins[0] == idx);
        let kind = if wins_now {
            Some(AttackKind::Five)
        } else if !stops_opponent {
            None
        } else if turn == BLACK {
            if self.makes_four_or_five(x, y, turn) && self.forbidden(x, y, true).is_none() {
                Some(self.attack_kind_after_move(x, y, turn))
            } else {
                None
            }
        } else {
            self.makes_four_or_five(x, y, turn)
                .then(|| self.attack_kind_after_move(x, y, turn))
        };

        let attack = kind.map(|kind| {
            let defenses = if kind == AttackKind::Four {
                self.vcf_defenses(turn, idx)
            } else {
                Vec::new()
            };
            VcfAttack {
                mv: idx,
                kind,
                defenses,
                score: self.priority(x, y, turn),
            }
        });
        self.board.clear(idx);
        attack
    }

    fn vcf_attacks_legacy(&mut self, turn: u8, opponent_wins: &[usize]) -> Vec<usize> {
        let key = AttackCacheKey {
            black: self.board.black,
            white: self.board.white,
            turn,
            opponent_wins: opponent_wins_signature(opponent_wins),
        };
        if let Some(cached) = self.legacy_attack_cache.get(&key) {
            return cached.clone();
        }

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
        let attacks: Vec<usize> = result.into_iter().map(|(idx, _)| idx).collect();
        clear_if_over_limit(&mut self.legacy_attack_cache, SEARCH_CACHE_SOFT_LIMIT);
        self.legacy_attack_cache.insert(key, attacks.clone());
        attacks
    }




    fn prioritize_pv(&self, attacks: &mut [VcfAttack], key: TtKey) {
        let Some(&pv_move) = self.pv.get(&key) else {
            return;
        };
        if let Some(index) = attacks.iter().position(|attack| attack.mv == pv_move) {
            attacks.swap(0, index);
        }
    }



    fn order_defenses(&mut self, defenses: &mut [usize], defender: u8, attack: usize) {
        let mut scored: Vec<(i32, usize)> = defenses
            .iter()
            .copied()
            .map(|idx| (self.defense_score(idx, defender, attack), idx))
            .collect();
        scored.sort_unstable_by_key(|&(score, idx)| (score, idx));

        for (dst, (_, idx)) in defenses.iter_mut().zip(scored) {
            *dst = idx;
        }
    }

    fn order_defenses_light(&self, defenses: &mut [usize], defender: u8, attack: usize) {
        let (ax, ay) = xy(attack);
        defenses.sort_by_key(|&idx| {
            let (x, y) = xy(idx);
            (x.abs_diff(ax) + y.abs_diff(ay)) as i32 - self.priority(x, y, defender)
        });
    }

    fn defense_score(&mut self, idx: usize, defender: u8, attack: usize) -> i32 {
        let (x, y) = xy(idx);
        self.board.set(idx, defender);
        let wins = self.check_win(x, y, defender);
        let makes_four = if wins {
            false
        } else {
            self.makes_four_or_five(x, y, defender)
        };
        self.board.clear(idx);

        if wins {
            -1_000_000
        } else if makes_four {
            -500_000 - self.priority(x, y, defender)
        } else {
            let (ax, ay) = xy(attack);
            (x.abs_diff(ax) + y.abs_diff(ay)) as i32 - self.priority(x, y, defender)
        }
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
        let key = ForbiddenKey {
            black: self.board.black,
            white: self.board.white,
            idx: idx as u8,
            check_three,
        };
        if let Some(&cached) = self.forbidden_cache.get(&key) {
            return cached;
        }

        let result = blackforbiddenmove::placed_forbidden(self.board.to_cells(), idx, check_three);
        clear_if_over_limit(&mut self.forbidden_cache, SEARCH_CACHE_SOFT_LIMIT);
        self.forbidden_cache.insert(key, result);
        result
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

    fn attack_kind_after_move(&mut self, x: usize, y: usize, color: u8) -> AttackKind {
        if self.valid_straight_four(x, y, color) {
            AttackKind::StraightFour
        } else {
            AttackKind::Four
        }
    }

    fn winning_moves(&mut self, turn: u8) -> Vec<usize> {
        let key = BoardColorKey {
            black: self.board.black,
            white: self.board.white,
            turn,
        };
        if let Some(cached) = self.winning_cache.get(&key) {
            return cached.clone();
        }

        let mut seen = [false; BOARD_CELLS];
        let mut wins = Vec::new();
        for idx in self.line_attack_candidates(turn, false) {
            if !self.board.is_empty(idx) {
                continue;
            }
            seen[idx] = true;
            let (x, y) = xy(idx);
            self.board.set(idx, turn);
            if self.check_win(x, y, turn) {
                wins.push(idx);
            }
            self.board.clear(idx);
        }
        for idx in 0..BOARD_CELLS {
            if seen[idx] || !self.board.is_empty(idx) {
                continue;
            }
            let (x, y) = xy(idx);
            self.board.set(idx, turn);
            if self.check_win(x, y, turn) {
                wins.push(idx);
            }
            self.board.clear(idx);
        }
        clear_if_over_limit(&mut self.winning_cache, SEARCH_CACHE_SOFT_LIMIT);
        self.winning_cache.insert(key, wins.clone());
        wins
    }



    pub(crate) fn legal_straight_four_on_current_board(&mut self, idx: usize, color: u8) -> bool {
        if idx >= BOARD_CELLS || !self.board.is_empty(idx) {
            return false;
        }
        let (x, y) = xy(idx);
        self.board.set(idx, color);
        let straight_four = self.valid_straight_four(x, y, color);
        let forbidden =
            color == BLACK && !self.check_win(x, y, color) && self.forbidden(x, y, true).is_some();
        self.board.clear(idx);
        straight_four && !forbidden
    }











    pub(crate) fn legal_non_straight_four_on_current_board(
        &mut self,
        idx: usize,
        color: u8,
    ) -> bool {
        if idx >= BOARD_CELLS || !self.board.is_empty(idx) {
            return false;
        }
        let (x, y) = xy(idx);
        self.board.set(idx, color);
        let valid = !self.check_win(x, y, color)
            && self.makes_four_or_five(x, y, color)
            && !self.valid_straight_four(x, y, color)
            && (color != BLACK || self.forbidden(x, y, true).is_none());
        self.board.clear(idx);
        valid
    }

    fn active_three_defenses_after_move(&mut self, attack: usize, color: u8) -> Vec<usize> {
        let (ax, ay) = xy(attack);
        let mut seen = [false; BOARD_CELLS];
        let mut defenses = Vec::new();

        for (dx, dy) in DIRS {
            for offset in -4..=4 {
                if offset == 0 {
                    continue;
                }
                let Some(idx) = index(ax as isize + dx * offset, ay as isize + dy * offset) else {
                    continue;
                };
                if seen[idx] || !self.board.is_empty(idx) {
                    continue;
                }
                seen[idx] = true;

                if !self.legal_straight_four_on_current_board(idx, color) {
                    continue;
                }

                self.board.clear(attack);
                let already_straight_four = self.legal_straight_four_on_current_board(idx, color);
                self.board.set(attack, color);
                if !already_straight_four {
                    defenses.push(idx);
                }
            }
        }

        defenses
    }


    pub(crate) fn four_move_has_simultaneous_active_three(
        &mut self,
        attack: usize,
        color: u8,
    ) -> bool {
        if attack >= BOARD_CELLS || !self.board.is_empty(attack) {
            return false;
        }
        let (x, y) = xy(attack);
        self.board.set(attack, color);
        let legal =
            color != BLACK || self.check_win(x, y, color) || self.forbidden(x, y, true).is_none();
        let active_three = legal && !self.active_three_defenses_after_move(attack, color).is_empty();
        self.board.clear(attack);
        active_three
    }





















    fn line_attack_candidates(&self, turn: u8, include_four_makers: bool) -> Vec<usize> {
        let mut seen = [false; BOARD_CELLS];
        let mut candidates = Vec::new();

        for window in five_point_windows() {
            let mut stones = 0;
            let mut empties = Vec::new();
            let mut blocked = false;

            for &idx in window {
                match self.board.get(idx) {
                    value if value == turn => stones += 1,
                    EMPTY => empties.push(idx),
                    _ => {
                        blocked = true;
                        break;
                    }
                }
            }
            if blocked {
                continue;
            }

            if stones == 4 && empties.len() == 1 {
                push_unique(&mut candidates, &mut seen, empties[0]);
            } else if include_four_makers && stones == 3 && empties.len() == 2 {
                for idx in empties {
                    push_unique(&mut candidates, &mut seen, idx);
                }
            }
        }

        candidates
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
                let Some(idx) = index(x as isize + ox, y as isize + oy) else {
                    continue;
                };
                if self.board.get(idx) == color {
                    score += 4;
                } else if self.board.get(idx) == other(color) {
                    score += 1;
                }
            }
        }
        score * 100 - (x.abs_diff(7) + y.abs_diff(7)) as i32
    }

    pub(super) fn put(&mut self, idx: usize, color: u8) {
        debug_assert!(self.board.is_empty(idx));
        self.bounds_stack.push(self.bounds);
        let (x, y) = xy(idx);
        self.bounds = Some(expand_bounds(self.bounds, x, y));
        self.board.set(idx, color);
        self.hash ^= self.zobrist[idx][color as usize];
    }

    pub(super) fn remove(&mut self, idx: usize, color: u8) {
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
    fn local_deadline_expired(&self) -> bool {
        self.local_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
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

fn five_point_windows() -> &'static Vec<[usize; 5]> {
    static WINDOWS: OnceLock<Vec<[usize; 5]>> = OnceLock::new();
    WINDOWS.get_or_init(|| {
        let mut windows = Vec::new();
        for (dx, dy) in DIRS {
            for y in 0..BOARD_SIZE {
                for x in 0..BOARD_SIZE {
                    let end_x = x as isize + dx * 4;
                    let end_y = y as isize + dy * 4;
                    if index(end_x, end_y).is_none() {
                        continue;
                    }

                    let mut window = [0; 5];
                    for step in 0..5 {
                        window[step as usize] =
                            index(x as isize + dx * step, y as isize + dy * step)
                                .expect("validated five-point line must stay on the board");
                    }
                    windows.push(window);
                }
            }
        }
        windows
    })
}

fn push_unique(items: &mut Vec<usize>, seen: &mut [bool; BOARD_CELLS], idx: usize) {
    if !seen[idx] {
        seen[idx] = true;
        items.push(idx);
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

/// Bound long-running search caches without periodically throwing away their
/// entire working set.  Removing a small arbitrary slice is intentionally
/// policy-free: every cache remains advisory and exact TT/proof results are
/// still recomputed when absent.  It avoids the large allocation and latency
/// spike caused by a full clear at the soft limit.
fn clear_if_over_limit<K: Clone + Eq + Hash, V>(map: &mut FxHashMap<K, V>, limit: usize) {
    if map.len() > limit {
        let evict = (limit / 8).max(1).min(map.len());
        let keys = map.keys().take(evict).cloned().collect::<Vec<_>>();
        for key in keys {
            map.remove(&key);
        }
    }
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

fn vcf_depth_schedule_for_mode(mode: SearchMode, max_depth: usize) -> Vec<usize> {
    match mode {
        SearchMode::Shortest => linear_depth_schedule(max_depth),
        SearchMode::Fast | SearchMode::FirstWin | SearchMode::Long => {
            let depth = if max_depth.is_multiple_of(2) {
                max_depth.saturating_sub(1).max(1)
            } else {
                max_depth
            };
            vec![depth]
        }
    }
}

fn vcf_threat_candidate_time_limit(remaining: Duration, candidates_left: usize) -> Duration {
    let candidates_left = candidates_left.max(1) as u128;
    let millis = (remaining.as_millis() / candidates_left).clamp(500, 10_000) as u64;
    Duration::from_millis(millis)
}

fn vcf_threat_quick_candidate_time_limit(total_limit: Duration) -> Duration {
    let millis = (total_limit.as_millis() / 500).clamp(10, 30) as u64;
    Duration::from_millis(millis)
}

#[cfg(target_os = "windows")]
fn vct_trace_log() -> &'static Mutex<VecDeque<String>> {
    VCT_TRACE_LOG.get_or_init(|| Mutex::new(VecDeque::with_capacity(VCT_TRACE_LOG_LIMIT)))
}

#[cfg(target_os = "windows")]
fn remember_vct_trace_line(line: String) {
    let Ok(mut log) = vct_trace_log().lock() else {
        return;
    };
    if log.len() >= VCT_TRACE_LOG_LIMIT {
        log.pop_front();
    }
    log.push_back(line);
}

#[cfg(target_os = "windows")]
pub fn vct_trace_snapshot() -> Vec<String> {
    vct_trace_log()
        .lock()
        .map(|log| log.iter().cloned().collect::<Vec<_>>())
        .unwrap_or_default()
}

#[cfg(target_os = "windows")]
pub fn clear_vct_trace_log() {
    if let Ok(mut log) = vct_trace_log().lock() {
        log.clear();
    }
}
