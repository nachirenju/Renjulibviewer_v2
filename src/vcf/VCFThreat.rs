#![allow(dead_code)] // Retained Threat-analysis helpers are shared test/debug API after VCT removal.

use super::{
    model::BOARD_CELLS,
    solver::{Move as VcfMove, SearchMode, Solver},
};
use std::{
    cell::RefCell,
    cmp::Reverse,
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use web_time::Instant;

const THREAT_DIRS: [(isize, isize); 4] = [(1, 0), (0, 1), (1, 1), (1, -1)];
const THREAT_FRONTIER_REFINE_LIMIT: usize = 4;
// Stage A intentionally stays cheap.  Refine only a bounded number of
// additional two-direction shapes: otherwise a crowded board turns this
// ordering hint into a full Solver pass for dozens of moves per node.
const THREAT_FOUR_THREE_SEED_REFINE_LIMIT: usize = 5;
const THREAT_INITIAL_MULTI_REFINE_LIMIT: usize = 3;
const THREAT_STAGE_A_CACHE_LIMIT: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ThreatStageAKey {
    lines: [u32; 4],
    neighborhood: u64,
    idx: u8,
    turn: u8,
    candidate_class: u8,
}

thread_local! {
    static THREAT_STAGE_A_CACHE: RefCell<HashMap<ThreatStageAKey, ThreatFrontier>> =
        RefCell::new(HashMap::new());
}

#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct ThreatFrontierStats {
    pub candidate_generation_calls: u64,
    pub candidate_generation_ns: u64,
    pub stage_a_candidates: u64,
    pub stage_a_ns: u64,
    pub refine_calls: u64,
    pub refine_ns: u64,
    pub solver_constructions: u64,
}

impl ThreatFrontierStats {
    pub(crate) fn delta_from(&self, before: &Self) -> Self {
        Self {
            candidate_generation_calls: self
                .candidate_generation_calls
                .saturating_sub(before.candidate_generation_calls),
            candidate_generation_ns: self
                .candidate_generation_ns
                .saturating_sub(before.candidate_generation_ns),
            stage_a_candidates: self.stage_a_candidates.saturating_sub(before.stage_a_candidates),
            stage_a_ns: self.stage_a_ns.saturating_sub(before.stage_a_ns),
            refine_calls: self.refine_calls.saturating_sub(before.refine_calls),
            refine_ns: self.refine_ns.saturating_sub(before.refine_ns),
            solver_constructions: self
                .solver_constructions
                .saturating_sub(before.solver_constructions),
        }
    }
}

static THREAT_CANDIDATE_GENERATION_CALLS: AtomicU64 = AtomicU64::new(0);
static THREAT_CANDIDATE_GENERATION_NS: AtomicU64 = AtomicU64::new(0);
static FRONTIER_STAGE_A_CANDIDATES: AtomicU64 = AtomicU64::new(0);
static FRONTIER_STAGE_A_NS: AtomicU64 = AtomicU64::new(0);
static FRONTIER_REFINE_CALLS: AtomicU64 = AtomicU64::new(0);
static FRONTIER_REFINE_NS: AtomicU64 = AtomicU64::new(0);
static FRONTIER_SOLVER_CONSTRUCTIONS: AtomicU64 = AtomicU64::new(0);

pub(crate) fn frontier_stats_snapshot() -> ThreatFrontierStats {
    ThreatFrontierStats {
        candidate_generation_calls: THREAT_CANDIDATE_GENERATION_CALLS.load(Ordering::Relaxed),
        candidate_generation_ns: THREAT_CANDIDATE_GENERATION_NS.load(Ordering::Relaxed),
        stage_a_candidates: FRONTIER_STAGE_A_CANDIDATES.load(Ordering::Relaxed),
        stage_a_ns: FRONTIER_STAGE_A_NS.load(Ordering::Relaxed),
        refine_calls: FRONTIER_REFINE_CALLS.load(Ordering::Relaxed),
        refine_ns: FRONTIER_REFINE_NS.load(Ordering::Relaxed),
        solver_constructions: FRONTIER_SOLVER_CONSTRUCTIONS.load(Ordering::Relaxed),
    }
}

fn elapsed_ns(started: Instant) -> u64 {
    started.elapsed().as_nanos().min(u64::MAX as u128) as u64
}

pub enum ThreatSearchOutcome {
    Complete(Vec<VcfMove>),
    Partial(Vec<VcfMove>),
    Cancelled,
}

pub fn find_vcf_threats(
    board: [u8; BOARD_CELLS],
    turn: u8,
    max_depth: usize,
    time_limit: Duration,
    mode: SearchMode,
    cancel: Option<Arc<AtomicBool>>,
) -> ThreatSearchOutcome {
    Solver::find_vcf_threats_reusing_cache(board, turn, max_depth, time_limit, mode, cancel)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ThreatFrontier {
    pub new_active_three_points: u8,
    pub new_active_three_directions: u8,
    pub new_multi_active_three_points: u8,
    pub new_four_points: u8,
    /// Follow-up points that newly make a four and an active three together.
    /// This detects quiet moves that leave multiple four-three continuations.
    pub new_four_three_points: u8,
    pub active_directions: u8,
    pub bridge_pairs: u8,
    pub strong_windows: u8,
    pub min_missing_to_four: u8,
    pub locality: u8,
    pub completeness_fallback: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ScoredThreatCandidate {
    pub idx: usize,
    pub frontier: ThreatFrontier,
    pub vcf_bridge: bool,
    pub vcf_attack_count: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ThreatFrontierClass {
    VerifiedVcfBridge,
    FourThreeSeed,
    MultiActiveThreeExpansion,
    ActiveThreeExpansion,
    MultiDirectionBridge,
    StrongPreparation,
    LocalPreparation,
    CompletenessFallback,
}

impl ScoredThreatCandidate {
    pub(crate) fn frontier_class(&self) -> ThreatFrontierClass {
        if self.vcf_bridge {
            ThreatFrontierClass::VerifiedVcfBridge
        } else if self.frontier.new_four_three_points >= 2 {
            ThreatFrontierClass::FourThreeSeed
        } else if self.frontier.new_multi_active_three_points > 0 {
            ThreatFrontierClass::MultiActiveThreeExpansion
        } else if self.frontier.new_active_three_points > 0 {
            ThreatFrontierClass::ActiveThreeExpansion
        } else if self.frontier.active_directions >= 2 && self.frontier.bridge_pairs > 0 {
            ThreatFrontierClass::MultiDirectionBridge
        } else if self.frontier.strong_windows >= 2 || self.frontier.min_missing_to_four <= 1 {
            ThreatFrontierClass::StrongPreparation
        } else if !self.frontier.completeness_fallback {
            ThreatFrontierClass::LocalPreparation
        } else {
            ThreatFrontierClass::CompletenessFallback
        }
    }

    pub(crate) fn initial_pn(&self, defense_mode_complete: bool) -> u32 {
        match self.frontier_class() {
            ThreatFrontierClass::VerifiedVcfBridge => 1,
            ThreatFrontierClass::FourThreeSeed => 2,
            ThreatFrontierClass::MultiActiveThreeExpansion => 2,
            ThreatFrontierClass::ActiveThreeExpansion => 3,
            ThreatFrontierClass::MultiDirectionBridge => 4,
            ThreatFrontierClass::StrongPreparation => 6,
            ThreatFrontierClass::LocalPreparation => {
                if defense_mode_complete {
                    8
                } else {
                    5
                }
            }
            ThreatFrontierClass::CompletenessFallback => {
                if defense_mode_complete {
                    16
                } else {
                    8
                }
            }
        }
    }
}

pub(crate) fn ordered_threat_candidates(
    board: &[u8; BOARD_CELLS],
    turn: u8,
) -> Vec<usize> {
    ordered_threat_candidates_scored_internal(board, turn, true, true)
        .into_iter()
        .map(|candidate| candidate.idx)
        .collect()
}

/// Candidate set used by df-pn. Membership remains complete, but ordering avoids
/// calling the expensive sleeping-three validator for every empty point.
pub(crate) fn ordered_threat_candidates_lazy(
    board: &[u8; BOARD_CELLS],
    turn: u8,
) -> Vec<usize> {
    ordered_threat_candidates_scored_internal(board, turn, false, true)
        .into_iter()
        .map(|candidate| candidate.idx)
        .collect()
}

pub(crate) fn ordered_threat_candidates_local(
    board: &[u8; BOARD_CELLS],
    turn: u8,
) -> Vec<usize> {
    ordered_threat_candidates_scored_internal(board, turn, false, false)
        .into_iter()
        .map(|candidate| candidate.idx)
        .collect()
}

pub(crate) fn ordered_threat_candidates_scored(
    board: &[u8; BOARD_CELLS],
    turn: u8,
    include_full_board_fallback: bool,
) -> Vec<ScoredThreatCandidate> {
    ordered_threat_candidates_scored_internal(board, turn, false, include_full_board_fallback)
}

/// Split the complete frontier into the locally meaningful candidates and the
/// remote completeness fallback.  The latter cannot carry an A/B certificate
/// on the current board, so DF-PN may retain just its coordinates until C is
/// actually opened.  Membership is unchanged.
pub(crate) fn ordered_threat_candidates_scored_partitioned(
    board: &[u8; BOARD_CELLS],
    turn: u8,
) -> (Vec<ScoredThreatCandidate>, Vec<usize>) {
    let generation_started = Instant::now();
    THREAT_CANDIDATE_GENERATION_CALLS.fetch_add(1, Ordering::Relaxed);
    let mut local = Vec::new();
    let mut fallback = Vec::new();
    for candidate in collect_threat_candidate_classes(board, turn, true) {
        if candidate.1 >= 2 {
            fallback.push(candidate.0);
        } else {
            local.push(candidate);
        }
    }
    let scored = score_threat_candidate_classes(board, turn, false, local, true);
    THREAT_CANDIDATE_GENERATION_NS.fetch_add(elapsed_ns(generation_started), Ordering::Relaxed);
    (scored, fallback)
}

/// Materialize deferred remote fallback coordinates only when FirstWin has
/// exhausted A and B (and any local C candidates).  This reproduces their
/// former stage-A scoring without the unrelated candidate scan.
pub(crate) fn score_deferred_threat_candidates(
    board: &[u8; BOARD_CELLS],
    turn: u8,
    points: Vec<usize>,
) -> Vec<ScoredThreatCandidate> {
    let candidates = points
        .into_iter()
        .filter(|&idx| idx < BOARD_CELLS && board[idx] == 0)
        .map(|idx| (idx, 2))
        .collect();
    score_threat_candidate_classes(board, turn, false, candidates, false)
}

fn ordered_threat_candidates_scored_internal(
    board: &[u8; BOARD_CELLS],
    turn: u8,
    classify_sleeping_three: bool,
    include_full_board_fallback: bool,
) -> Vec<ScoredThreatCandidate> {
    let generation_started = Instant::now();
    THREAT_CANDIDATE_GENERATION_CALLS.fetch_add(1, Ordering::Relaxed);
    let candidates = collect_threat_candidate_classes(board, turn, include_full_board_fallback);
    let scored = score_threat_candidate_classes(
        board,
        turn,
        classify_sleeping_three,
        candidates,
        false,
    );
    THREAT_CANDIDATE_GENERATION_NS.fetch_add(elapsed_ns(generation_started), Ordering::Relaxed);
    scored
}

fn score_threat_candidate_classes(
    board: &[u8; BOARD_CELLS],
    turn: u8,
    classify_sleeping_three: bool,
    candidates: Vec<(usize, u8)>,
    initial_multi_only: bool,
) -> Vec<ScoredThreatCandidate> {
    let mut scored = Vec::with_capacity(candidates.len());
    let mut sleeping_solver = classify_sleeping_three.then(|| frontier_solver(*board));
    let stage_a_started = Instant::now();
    for (idx, candidate_class) in candidates {
        let mut frontier = stage_a_frontier(board, idx, turn, candidate_class);
        if sleeping_solver.as_mut().is_some_and(|solver| {
            move_creates_sleeping_three_with_solver(board, idx, turn, solver)
        }) {
            frontier.strong_windows = frontier.strong_windows.saturating_add(1);
        }
        scored.push(ScoredThreatCandidate {
            idx,
            frontier,
            vcf_bridge: false,
            vcf_attack_count: 0,
        });
    }
    FRONTIER_STAGE_A_CANDIDATES.fetch_add(scored.len() as u64, Ordering::Relaxed);
    FRONTIER_STAGE_A_NS.fetch_add(elapsed_ns(stage_a_started), Ordering::Relaxed);

    scored.sort_by_key(threat_frontier_order_key);
    let refine_limit = scored.len().min(THREAT_FRONTIER_REFINE_LIMIT);
    let mut extra_seed_refinements = 0;
    let mut initial_multi_refinements = 0;
    for (rank, candidate) in scored.iter_mut().enumerate() {
        // A quiet two-direction preparation can sit just outside the initial
        // local frontier. Refine it as well when it is within two stones of a
        // four in both directions; this is still a small local subset.
        let four_three_seed_shape = candidate.frontier.active_directions >= 2
            && candidate.frontier.min_missing_to_four <= 2;
        let in_primary_frontier = rank < refine_limit;
        if initial_multi_only {
            if !four_three_seed_shape
                || initial_multi_refinements >= THREAT_INITIAL_MULTI_REFINE_LIMIT
            {
                continue;
            }
            initial_multi_refinements += 1;
        }
        let use_seed_refinement = !in_primary_frontier
            && four_three_seed_shape
            && extra_seed_refinements < THREAT_FOUR_THREE_SEED_REFINE_LIMIT;
        if !initial_multi_only && !in_primary_frontier && !use_seed_refinement {
            continue;
        }
        if use_seed_refinement {
            extra_seed_refinements += 1;
        }
        let refine_started = Instant::now();
        FRONTIER_REFINE_CALLS.fetch_add(1, Ordering::Relaxed);
        refine_frontier_candidate(board, turn, candidate);
        FRONTIER_REFINE_NS.fetch_add(elapsed_ns(refine_started), Ordering::Relaxed);
    }
    scored.sort_by_key(threat_frontier_order_key);
    scored
}

/// Exact refinement used only when the deferred ordinary-Threat phase is
/// actually opened.  The initial pass pays for a few simultaneous shapes;
/// single Threats remain cheap coordinates until this function is called.
pub(crate) fn refine_scored_threat_candidate(
    board: &[u8; BOARD_CELLS],
    turn: u8,
    candidate: &mut ScoredThreatCandidate,
) {
    let started = Instant::now();
    FRONTIER_REFINE_CALLS.fetch_add(1, Ordering::Relaxed);
    refine_frontier_candidate(board, turn, candidate);
    FRONTIER_REFINE_NS.fetch_add(elapsed_ns(started), Ordering::Relaxed);
}

fn threat_frontier_order_key(
    candidate: &ScoredThreatCandidate,
) -> (
    ThreatFrontierClass,
    Reverse<u8>,
    Reverse<u8>,
    Reverse<u8>,
    Reverse<u8>,
    Reverse<u8>,
    Reverse<u8>,
    Reverse<u8>,
    Reverse<u8>,
    u8,
    Reverse<u8>,
    usize,
) {
    let f = candidate.frontier;
    (
        candidate.frontier_class(),
        Reverse(f.new_multi_active_three_points),
        Reverse(f.new_active_three_points),
        Reverse(f.new_active_three_directions),
        Reverse(candidate.vcf_attack_count),
        Reverse((f.new_four_three_points << 4) | f.new_four_points.min(15)),
        Reverse(f.active_directions),
        Reverse(f.bridge_pairs),
        Reverse(f.strong_windows),
        f.min_missing_to_four,
        Reverse(f.locality),
        candidate.idx,
    )
}

fn collect_threat_candidate_classes(
    board: &[u8; BOARD_CELLS],
    turn: u8,
    include_full_board_fallback: bool,
) -> Vec<(usize, u8)> {
    let mut seen = [false; BOARD_CELLS];
    let mut candidate_class = [u8::MAX; BOARD_CELLS];
    let mut candidates = Vec::new();

    // Class 0: points in an unblocked five-point window containing own stones.
    for (dx, dy) in THREAT_DIRS {
        for y in 0..15 {
            for x in 0..15 {
                let end_x = x as isize + dx * 4;
                let end_y = y as isize + dy * 4;
                if threat_index(end_x, end_y).is_none() {
                    continue;
                }

                let mut own_stones = 0;
                // A five-window contains at most five empty points.  Keeping
                // them on the stack avoids an allocation for every scanned
                // window in the complete threat frontier.
                let mut empties = [0usize; 5];
                let mut empty_count = 0usize;
                let mut blocked = false;
                for step in 0..5 {
                    let idx = threat_index(x as isize + dx * step, y as isize + dy * step)
                        .expect("validated threat line must stay on board");
                    match board[idx] {
                        value if value == turn => own_stones += 1,
                        0 => {
                            empties[empty_count] = idx;
                            empty_count += 1;
                        }
                        _ => {
                            blocked = true;
                            break;
                        }
                    }
                }

                if blocked || own_stones == 0 {
                    continue;
                }

                for &idx in &empties[..empty_count] {
                    candidate_class[idx] = 0;
                    if !seen[idx] {
                        seen[idx] = true;
                        candidates.push(idx);
                    }
                }
            }
        }
    }

    // Class 1: nearby support/preparation points. This keeps E6/E10-like moves
    // ahead of remote completeness fallbacks without excluding any empty point.
    for idx in 0..BOARD_CELLS {
        if board[idx] != 0 || seen[idx] {
            continue;
        }
        let x = idx % 15;
        let y = idx / 15;
        if has_neighbor_stone(board, x, y, 2) {
            seen[idx] = true;
            candidate_class[idx] = 1;
            candidates.push(idx);
        }
    }

    if include_full_board_fallback {
        // Class 2: completeness fallback.
        for idx in 0..BOARD_CELLS {
            if board[idx] == 0 && !seen[idx] {
                seen[idx] = true;
                candidate_class[idx] = 2;
                candidates.push(idx);
            }
        }
    }

    candidates
        .into_iter()
        .map(|idx| (idx, candidate_class[idx]))
        .collect()
}

fn threat_stage_a_key(
    board: &[u8; BOARD_CELLS],
    idx: usize,
    turn: u8,
    candidate_class: u8,
) -> ThreatStageAKey {
    let x = idx % 15;
    let y = idx / 15;
    let mut lines = [0u32; 4];
    for (direction, (dx, dy)) in THREAT_DIRS.iter().copied().enumerate() {
        let mut encoded = 0u32;
        for offset in -7isize..=7 {
            let value = threat_index(x as isize + dx * offset, y as isize + dy * offset)
                .map(|point| board[point])
                .unwrap_or(3);
            encoded |= (value as u32) << ((offset + 7) as u32 * 2);
        }
        lines[direction] = encoded;
    }
    let mut neighborhood = 0u64;
    let mut shift = 0u32;
    for ny in y.saturating_sub(2)..=(y + 2).min(14) {
        for nx in x.saturating_sub(2)..=(x + 2).min(14) {
            if nx == x && ny == y {
                continue;
            }
            neighborhood |= (board[ny * 15 + nx] as u64) << shift;
            shift += 2;
        }
    }
    ThreatStageAKey {
        lines,
        neighborhood,
        idx: idx as u8,
        turn,
        candidate_class,
    }
}

fn stage_a_frontier(
    board: &[u8; BOARD_CELLS],
    idx: usize,
    turn: u8,
    candidate_class: u8,
) -> ThreatFrontier {
    let cache_key = threat_stage_a_key(board, idx, turn, candidate_class);
    if let Some(cached) = THREAT_STAGE_A_CACHE.with(|cache| cache.borrow().get(&cache_key).copied())
    {
        return cached;
    }
    let x = idx % 15;
    let y = idx / 15;
    let mut frontier = ThreatFrontier {
        min_missing_to_four: 5,
        completeness_fallback: candidate_class >= 2,
        locality: locality_score(board, x, y, turn),
        ..ThreatFrontier::default()
    };

    let mut active_mask = 0u8;
    for (dir_idx, (dx, dy)) in THREAT_DIRS.iter().copied().enumerate() {
        let mut own_window_count = 0u8;
        let mut directional_strong = false;
        let mut best_missing = 5u8;

        for start in -4..=0 {
            let mut own = 0u8;
            let mut blocked = false;
            let mut includes_candidate = false;

            for step in 0..5 {
                let tx = x as isize + (start + step) * dx;
                let ty = y as isize + (start + step) * dy;
                let Some(pos) = threat_index(tx, ty) else {
                    blocked = true;
                    break;
                };
                if pos == idx {
                    includes_candidate = true;
                    continue;
                }
                match board[pos] {
                    value if value == turn => own += 1,
                    0 => {}
                    _ => {
                        blocked = true;
                        break;
                    }
                }
            }

            if blocked || !includes_candidate || own == 0 {
                continue;
            }

            own_window_count = own_window_count.saturating_add(1);
            let missing_to_four = 4u8.saturating_sub(own.saturating_add(1));
            best_missing = best_missing.min(missing_to_four);
            if own >= 2 {
                directional_strong = true;
                frontier.strong_windows = frontier.strong_windows.saturating_add(1);
            }
        }

        if own_window_count > 0 {
            active_mask |= 1 << dir_idx;
        }
        if directional_strong {
            frontier.bridge_pairs = frontier
                .bridge_pairs
                .saturating_add(own_window_count.saturating_sub(1));
        }
        frontier.min_missing_to_four = frontier.min_missing_to_four.min(best_missing);
    }

    frontier.active_directions = active_mask.count_ones() as u8;
    THREAT_STAGE_A_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= THREAT_STAGE_A_CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(cache_key, frontier);
    });
    frontier
}

fn refine_frontier_candidate(
    board: &[u8; BOARD_CELLS],
    turn: u8,
    candidate: &mut ScoredThreatCandidate,
) {
    let idx = candidate.idx;
    if idx >= BOARD_CELLS || board[idx] != 0 {
        return;
    }

    let mut after = *board;
    after[idx] = turn;
    // One mutable workspace is enough: record the exact pre-move facts,
    // place the candidate, and query the same Solver again. This preserves
    // forbidden-move semantics while retaining every local legality cache.
    let mut solver = frontier_solver(*board);
    let affected = affected_frontier_points(idx);
    let mut direction_union = 0u8;
    let mut before_active = [0u8; BOARD_CELLS];
    let mut before_four = [false; BOARD_CELLS];
    let mut before_four_three = [false; BOARD_CELLS];

    for &point in &affected {
        if board[point] != 0 {
            continue;
        }
        before_active[point] = solver.active_three_direction_mask_on_current_board(point, turn);
        before_four[point] = solver.legal_non_straight_four_on_current_board(point, turn);
        before_four_three[point] =
            solver.four_move_has_simultaneous_active_three(point, turn);
    }
    solver.put(idx, turn);

    for &point in &affected {
        if after[point] != 0 {
            continue;
        }
        let after_mask = solver.active_three_direction_mask_on_current_board(point, turn);
        if before_active[point] == 0 && after_mask != 0 {
            candidate.frontier.new_active_three_points =
                candidate.frontier.new_active_three_points.saturating_add(1);
            direction_union |= after_mask;
            if after_mask.count_ones() >= 2 {
                candidate.frontier.new_multi_active_three_points = candidate
                    .frontier
                    .new_multi_active_three_points
                    .saturating_add(1);
            }
        }

        let after_four = solver.legal_non_straight_four_on_current_board(point, turn);
        if after_four && !before_four[point] {
            candidate.frontier.new_four_points =
                candidate.frontier.new_four_points.saturating_add(1);
        }

        let after_four_three = solver.four_move_has_simultaneous_active_three(point, turn);
        if after_four_three && !before_four_three[point] {
            candidate.frontier.new_four_three_points = candidate
                .frontier
                .new_four_three_points
                .saturating_add(1);
        }
    }

    candidate.frontier.new_active_three_directions = direction_union.count_ones() as u8;
    candidate.vcf_attack_count = cheap_vcf_attack_count(&mut solver, turn, &affected);
    solver.remove(idx, turn);
    candidate.vcf_bridge = candidate.vcf_attack_count > 0
        || candidate.frontier.new_four_points > 0
        || candidate.frontier.new_four_three_points >= 2
        || candidate.frontier.new_active_three_points >= 2;
}

pub(crate) fn active_three_direction_mask(
    board: [u8; BOARD_CELLS],
    idx: usize,
    color: u8,
) -> u8 {
    frontier_solver(board).active_three_direction_mask_on_current_board(idx, color)
}

fn cheap_vcf_attack_count(solver: &mut Solver, turn: u8, affected: &[usize]) -> u8 {
    let mut count = 0u8;
    for &point in affected {
        let straight_four = solver.legal_straight_four_on_current_board(point, turn);
        let four = solver.legal_non_straight_four_on_current_board(point, turn);
        if straight_four || four {
            count = count.saturating_add(1);
        }
    }
    count
}

fn frontier_solver(board: [u8; BOARD_CELLS]) -> Solver {
    FRONTIER_SOLVER_CONSTRUCTIONS.fetch_add(1, Ordering::Relaxed);
    Solver::new(board, Duration::from_secs(1), SearchMode::Fast)
}

#[cfg(test)]
fn frontier_legal_straight_four_after(
    board: [u8; BOARD_CELLS],
    idx: usize,
    color: u8,
) -> Result<bool, String> {
    let mut solver = frontier_solver(board);
    Ok(solver.legal_straight_four_on_current_board(idx, color))
}

fn affected_frontier_points(idx: usize) -> Vec<usize> {
    let x = idx % 15;
    let y = idx / 15;
    let mut seen = [false; BOARD_CELLS];
    let mut points = Vec::new();
    for (dx, dy) in THREAT_DIRS {
        for offset in -8..=8 {
            if offset == 0 {
                continue;
            }
            let Some(point) = threat_index(x as isize + dx * offset, y as isize + dy * offset)
            else {
                continue;
            };
            if !seen[point] {
                seen[point] = true;
                points.push(point);
            }
        }
    }
    points
}

fn locality_score(board: &[u8; BOARD_CELLS], x: usize, y: usize, turn: u8) -> u8 {
    let mut own = 0u8;
    let mut opponent = 0u8;
    let mut nearest = 15u8;

    for ny in y.saturating_sub(2)..=(y + 2).min(14) {
        for nx in x.saturating_sub(2)..=(x + 2).min(14) {
            if nx == x && ny == y {
                continue;
            }
            let stone = board[ny * 15 + nx];
            if stone == 0 {
                continue;
            }
            nearest = nearest.min((x.abs_diff(nx) + y.abs_diff(ny)) as u8);
            if stone == turn {
                own = own.saturating_add(1);
            } else {
                opponent = opponent.saturating_add(1);
            }
        }
    }

    let center_bonus = 14u8.saturating_sub((x.abs_diff(7) + y.abs_diff(7)) as u8);
    own.saturating_mul(3)
        .saturating_add(center_bonus / 2)
        .saturating_add(5u8.saturating_sub(nearest.min(5)))
        .saturating_sub(opponent.saturating_mul(2))
}

fn has_neighbor_stone(board: &[u8; BOARD_CELLS], x: usize, y: usize, radius: usize) -> bool {
    let min_x = x.saturating_sub(radius);
    let max_x = (x + radius).min(14);
    let min_y = y.saturating_sub(radius);
    let max_y = (y + radius).min(14);

    for ny in min_y..=max_y {
        for nx in min_x..=max_x {
            if nx == x && ny == y {
                continue;
            }
            if board[ny * 15 + nx] != 0 {
                return true;
            }
        }
    }

    false
}

fn threat_index(x: isize, y: isize) -> Option<usize> {
    if (0..15).contains(&x) && (0..15).contains(&y) {
        Some(y as usize * 15 + x as usize)
    } else {
        None
    }
}

pub(crate) fn move_creates_sleeping_three(
    board: [u8; BOARD_CELLS],
    idx: usize,
    color: u8,
) -> bool {
    let mut solver = frontier_solver(board);
    move_creates_sleeping_three_with_solver(&board, idx, color, &mut solver)
}

fn move_creates_sleeping_three_with_solver(
    board: &[u8; BOARD_CELLS],
    idx: usize,
    color: u8,
    solver: &mut Solver,
) -> bool {
    if idx >= BOARD_CELLS || board[idx] != 0 {
        return false;
    }

    let x = idx % 15;
    let y = idx / 15;
    let mut after_move = *board;
    after_move[idx] = color;
    solver.put(idx, color);
    let mut found = false;

    'directions: for (dx, dy) in THREAT_DIRS {
        for start in -4..=0 {
            let mut stones = 0;
            let mut empties = [0usize; 5];
            let mut empty_count = 0usize;
            let mut includes_move = false;
            let mut blocked = false;

            for step in 0..5 {
                let offset = start + step;
                let tx = x as isize + offset * dx;
                let ty = y as isize + offset * dy;
                let Some(pos) = threat_index(tx, ty) else {
                    blocked = true;
                    break;
                };

                if pos == idx {
                    includes_move = true;
                }

                match after_move[pos] {
                    value if value == color => stones += 1,
                    0 => {
                        empties[empty_count] = pos;
                        empty_count += 1;
                    }
                    _ => {
                        blocked = true;
                        break;
                    }
                }
            }

            if blocked || !includes_move || stones != 3 || empty_count != 2 {
                continue;
            }

            for gap in &empties[..empty_count] {
                if solver.legal_non_straight_four_on_current_board(*gap, color) {
                    found = true;
                    break 'directions;
                }
            }
        }
    }

    solver.remove(idx, color);
    found
}

pub(crate) fn is_vcf_threat_candidate(
    board: [u8; BOARD_CELLS],
    idx: usize,
    color: u8,
) -> bool {
    if idx >= BOARD_CELLS || board[idx] != 0 {
        return false;
    }

    // Both checks restore the board.  Sharing one solver preserves their
    // exact semantics while reusing the position, forbidden-move and active
    // range caches that the legality check has just populated.
    let mut solver = frontier_solver(board);
    solver.is_legal_non_four_on_current_board(idx, color)
        && solver.active_three_direction_mask_on_current_board(idx, color) == 0
}

pub(crate) fn move_creates_active_three(
    board: [u8; BOARD_CELLS],
    idx: usize,
    color: u8,
) -> bool {
    active_three_direction_mask(board, idx, color) != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcf::solver::BLACK;

    fn idx(col: char, row: usize) -> usize {
        let x = (col as u8 - b'A') as usize;
        let y = 15 - row;
        y * 15 + x
    }

    fn move_creates_active_three_full_scan(
        board: [u8; BOARD_CELLS],
        idx: usize,
        color: u8,
    ) -> bool {
        if idx >= BOARD_CELLS || board[idx] != 0 {
            return false;
        }

        let mut after_move = board;
        after_move[idx] = color;

        for next_idx in 0..BOARD_CELLS {
            if after_move[next_idx] != 0 {
                continue;
            }
            let Ok(straight_four) =
                frontier_legal_straight_four_after(after_move, next_idx, color)
            else {
                continue;
            };
            if !straight_four {
                continue;
            }

            let already_straight_four =
                frontier_legal_straight_four_after(board, next_idx, color)
                    .is_ok_and(|value| value);
            if !already_straight_four {
                return true;
            }
        }

        false
    }

    #[test]
    fn excludes_move_that_directly_makes_four_from_vcf_threats() {
        let mut board = [0; BOARD_CELLS];
        board[7 * 15 + 4] = BLACK;
        board[7 * 15 + 5] = BLACK;
        board[7 * 15 + 6] = BLACK;

        assert!(!is_vcf_threat_candidate(board, 7 * 15 + 7, BLACK));
    }

    #[test]
    fn active_three_move_is_not_a_vcf_threat_candidate() {
        let mut board = [0; BOARD_CELLS];
        board[7 * 15 + 5] = BLACK;
        board[7 * 15 + 7] = BLACK;
        board[0] = 2;
        board[1] = 2;

        assert!(!is_vcf_threat_candidate(board, 7 * 15 + 6, BLACK));
    }

    #[test]
    fn localized_active_three_scan_matches_full_scan_on_known_candidates() {
        let mut board = [0; BOARD_CELLS];
        for point in [('F', 8), ('H', 8), ('H', 6), ('I', 6), ('G', 5)] {
            board[idx(point.0, point.1)] = BLACK;
        }

        for candidate in [idx('G', 8), idx('H', 7), idx('G', 6), idx('I', 7)] {
            assert_eq!(
                move_creates_active_three(board, candidate, BLACK),
                move_creates_active_three_full_scan(board, candidate, BLACK),
                "candidate {candidate} should match full active-three scan"
            );
        }
    }

    #[test]
    fn lazy_ordered_candidates_keep_the_same_complete_membership() {
        let mut board = [0; BOARD_CELLS];
        board[idx('H', 8)] = BLACK;
        board[idx('G', 8)] = 2;

        let complete = ordered_threat_candidates(&board, BLACK);
        let lazy = ordered_threat_candidates_lazy(&board, BLACK);
        let mut complete_sorted = complete.clone();
        let mut lazy_sorted = lazy.clone();
        complete_sorted.sort_unstable();
        lazy_sorted.sort_unstable();
        assert_eq!(lazy_sorted, complete_sorted);
    }

    #[test]
    fn partitioned_scored_candidates_keep_the_complete_frontier_membership() {
        let mut board = [0; BOARD_CELLS];
        board[idx('H', 8)] = BLACK;
        board[idx('G', 8)] = 2;

        let complete = ordered_threat_candidates(&board, BLACK);
        let (local, deferred) = ordered_threat_candidates_scored_partitioned(&board, BLACK);
        let mut partitioned = local.into_iter().map(|candidate| candidate.idx).collect::<Vec<_>>();
        partitioned.extend(deferred);
        partitioned.sort_unstable();
        let mut expected = complete;
        expected.sort_unstable();

        assert_eq!(partitioned, expected);
    }

    #[test]
    fn local_ordered_candidates_skip_remote_completeness_fallback() {
        let mut board = [0; BOARD_CELLS];
        board[idx('H', 8)] = BLACK;
        board[idx('G', 8)] = 2;

        let local = ordered_threat_candidates_local(&board, BLACK);
        assert!(local.contains(&idx('I', 8)));
        assert!(!local.contains(&idx('A', 1)));
        assert!(local.len() < board.iter().filter(|&&stone| stone == 0).count());
    }

    #[test]
    fn ordered_candidates_include_every_empty_point() {
        let mut board = [0; BOARD_CELLS];
        board[idx('H', 8)] = BLACK;
        board[idx('G', 8)] = 2;

        let candidates = ordered_threat_candidates(&board, BLACK);
        let mut seen = [false; BOARD_CELLS];
        for candidate in candidates {
            assert_eq!(board[candidate], 0);
            assert!(!seen[candidate], "candidate {candidate} must not be duplicated");
            seen[candidate] = true;
        }

        for point in 0..BOARD_CELLS {
            assert_eq!(seen[point], board[point] == 0, "point {point}");
        }
    }

    #[test]
    fn sleeping_three_filter_keeps_known_threat_candidates() {
        let mut board = [0; BOARD_CELLS];
        for point in [('H', 8), ('H', 7), ('H', 6), ('I', 6)] {
            board[idx(point.0, point.1)] = BLACK;
        }
        for point in [('H', 9), ('I', 8), ('I', 7), ('H', 10)] {
            board[idx(point.0, point.1)] = 2;
        }

        let candidates = ordered_threat_candidates(&board, BLACK);
        assert!(candidates.iter().all(|&candidate| board[candidate] == 0));
        assert!(candidates
            .iter()
            .any(|&candidate| move_creates_sleeping_three(board, candidate, BLACK)));
    }
}
