//! VCF search entry points.
//!
//! This module owns VCF-facing orchestration.  `solver.rs` deliberately only
//! exposes the stable API and delegates here; board/rule primitives remain on
//! `Solver` because they are shared with VCT.

use super::*;

pub(super) fn solve(
    board: [u8; BOARD_CELLS],
    turn: u8,
    max_depth: usize,
    time_limit: Duration,
    cancel: Arc<AtomicBool>,
    mode: SearchMode,
) -> SolveOutcome {
    let mut solver = Solver::new_with_cancel(board, time_limit, cancel, mode);
    let empty_cells = BOARD_CELLS - board.iter().filter(|&&stone| stone != EMPTY).count();
    let effective_depth = max_depth.min(empty_cells).max(1);
    let depths = vcf_depth_schedule_for_mode(mode, effective_depth);
    let mut saw_incomplete = false;
    for depth in depths {
        match solver.solve_at(turn, 0, depth) {
            Search::Found(path) => return SolveOutcome::Found(path),
            Search::Timeout => return SolveOutcome::Timeout,
            Search::Cancelled => return SolveOutcome::Cancelled,
            Search::Incomplete | Search::Backoff => {
                saw_incomplete = true;
                if mode == SearchMode::Shortest {
                    return SolveOutcome::Incomplete;
                }
            }
            Search::NotFound => {}
        }
    }
    if saw_incomplete {
        SolveOutcome::Incomplete
    } else {
        SolveOutcome::NotFound
    }
}

pub(super) fn solve_shortest(
    board: [u8; BOARD_CELLS],
    turn: u8,
    max_depth: usize,
    time_limit: Duration,
    cancel: Arc<AtomicBool>,
) -> SolveOutcome {
    let mut solver = Solver::new_with_cancel(board, time_limit, cancel, SearchMode::Shortest);
    let empty_cells = BOARD_CELLS - board.iter().filter(|&&stone| stone != EMPTY).count();
    let effective_depth = max_depth.min(empty_cells).max(1);
    for depth in linear_depth_schedule(effective_depth) {
        match solver.solve_at(turn, 0, depth) {
            Search::Found(path) => return SolveOutcome::Found(path),
            Search::Timeout => return SolveOutcome::Timeout,
            Search::Cancelled => return SolveOutcome::Cancelled,
            Search::Incomplete | Search::Backoff => return SolveOutcome::Incomplete,
            Search::NotFound => {}
        }
    }
    SolveOutcome::NotFound
}

pub(super) fn find_threats(
    board: [u8; BOARD_CELLS],
    turn: u8,
    max_depth: usize,
    time_limit: Duration,
    mode: SearchMode,
    cancel: Option<Arc<AtomicBool>>,
) -> vcf_threat::ThreatSearchOutcome {
    Solver::find_vcf_threats_engine(board, turn, max_depth, time_limit, mode, cancel)
}
