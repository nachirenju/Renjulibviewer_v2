//! Quintet DF-PNS VCT entry points.
//!
//! VCT intentionally uses Quintet's own potential-three candidate filter and
//! pass-VCF Threat definition. The application's VCF and Threat viewer APIs
//! remain in `VCF.rs` / `VCFThreat.rs` and are not used or changed here.
//!
//! Quintet is MIT licensed; its required notice is in `THIRD_PARTY_NOTICES`.

use super::*;

/// Quintet's attacker VCF depth used for pass-VCF Threat generation in VCT.
const QUINTET_THREAT_VCF_DEPTH: u8 = 20;

#[inline]
fn quintet_attacker_limit(max_depth: usize, empty_cells: usize) -> u8 {
    max_depth
        .min(empty_cells)
        .saturating_add(1)
        .saturating_div(2)
        .clamp(1, u8::MAX as usize) as u8
}

fn moves_from_quintet_points(
    points: &[quintet::board::Point],
    turn: u8,
) -> Option<Vec<Move>> {
    let mut path = Vec::with_capacity(points.len());
    for (ply, point) in points.iter().enumerate() {
        let (x, y) = (point.0 as usize, point.1 as usize);
        if x >= BOARD_SIZE || y >= BOARD_SIZE {
            return None;
        }
        path.push(Move {
            x,
            y,
            color: if ply % 2 == 0 { turn } else { other(turn) },
        });
    }
    Some(path)
}

fn solve_quintet_dfpn(
    board: [u8; BOARD_CELLS],
    turn: u8,
    max_depth: usize,
    time_limit: Duration,
    cancel: Arc<AtomicBool>,
    mut progress: Option<Box<dyn FnMut(Vec<Move>) + Send>>,
) -> Result<Option<Vec<Move>>, SolveOutcome> {
    if cancel.load(Ordering::Relaxed) {
        return Err(SolveOutcome::Cancelled);
    }
    if turn != BLACK && turn != WHITE {
        return Ok(None);
    }

    let mut blacks = Vec::new();
    let mut whites = Vec::new();
    for (idx, stone) in board.into_iter().enumerate() {
        match stone {
            BLACK => blacks.push(quintet::board::Point(
                (idx % BOARD_SIZE) as u8,
                (idx / BOARD_SIZE) as u8,
            )),
            WHITE => whites.push(quintet::board::Point(
                (idx % BOARD_SIZE) as u8,
                (idx / BOARD_SIZE) as u8,
            )),
            _ => {}
        }
    }
    let empty_cells = BOARD_CELLS - blacks.len() - whites.len();
    if empty_cells == 0 {
        return Ok(None);
    }

    let quintet_board = quintet::board::Board::from_stones(
        &quintet::board::Points(blacks),
        &quintet::board::Points(whites),
    );
    let started = Instant::now();
    let deadline = started + time_limit;
    let root_turn = turn;
    let result = quintet::mate::solve_vct_dfpns_with_control(
        quintet_attacker_limit(max_depth, empty_cells),
        &quintet_board,
        quintet::board::Player::from(turn == BLACK),
        QUINTET_THREAT_VCF_DEPTH,
        move || {
            if cancel.load(Ordering::Relaxed) {
                Some(quintet::mate::VCTSearchStop::Cancelled)
            } else if Instant::now() >= deadline {
                Some(quintet::mate::VCTSearchStop::TimedOut)
            } else {
                None
            }
        },
        move |snapshot| {
            let Some(path) = moves_from_quintet_points(&snapshot.path, root_turn) else {
                return;
            };
            if let Some(callback) = progress.as_mut() {
                callback(path);
            }
        },
    );
    let stats = match &result {
        quintet::mate::VCTSearchResult::Completed(_, stats)
        | quintet::mate::VCTSearchResult::Stopped(_, stats) => *stats,
    };
    #[cfg(target_os = "windows")]
    remember_vct_trace_line(format!(
        "[quintet-vct-candidates] attack_lists={} attack_before_dedup={} attack_after_dedup={} defence_lists={} defence_before_dedup={} defence_after_dedup={} positive_vcf_cache_hits={}",
        stats.attack_candidate_lists,
        stats.attack_candidates_before_dedup,
        stats.attack_candidates_after_dedup,
        stats.defence_candidate_lists,
        stats.defence_candidates_before_dedup,
        stats.defence_candidates_after_dedup,
        stats.positive_vcf_cache_hits,
    ));
    #[cfg(target_os = "windows")]
    remember_vct_trace_line(format!(
        "[quintet-vct-selector] attack_full_scans={} attack_cache_hits={} attack_incremental_updates={} attack_child_lookups={} defence_full_scans={} defence_cache_hits={} defence_incremental_updates={} defence_child_lookups={}",
        stats.attack_selector_full_scans,
        stats.attack_selector_cache_hits,
        stats.attack_selector_incremental_updates,
        stats.attack_selector_child_lookups,
        stats.defence_selector_full_scans,
        stats.defence_selector_cache_hits,
        stats.defence_selector_incremental_updates,
        stats.defence_selector_child_lookups,
    ));
    #[cfg(not(target_os = "windows"))]
    let _ = stats;
    match result {
        quintet::mate::VCTSearchResult::Completed(Some(mate), _) => {
            Ok(moves_from_quintet_points(mate.path(), turn).filter(|path| !path.is_empty()))
        }
        quintet::mate::VCTSearchResult::Completed(None, _) => Ok(None),
        quintet::mate::VCTSearchResult::Stopped(
            quintet::mate::VCTSearchStop::Cancelled,
            _,
        ) => {
            Err(SolveOutcome::Cancelled)
        }
        quintet::mate::VCTSearchResult::Stopped(
            quintet::mate::VCTSearchStop::TimedOut,
            _,
        ) => {
            Err(SolveOutcome::Timeout)
        }
    }
}

fn solve_internal(
    board: [u8; BOARD_CELLS],
    turn: u8,
    max_depth: usize,
    time_limit: Duration,
    cancel: Arc<AtomicBool>,
    progress: Option<Box<dyn FnMut(Vec<Move>) + Send>>,
) -> SolveOutcome {
    let started = Instant::now();
    let result = solve_quintet_dfpn(
        board,
        turn,
        max_depth,
        time_limit,
        cancel,
        progress,
    );
    let elapsed = started.elapsed().as_millis();
    #[cfg(not(target_os = "windows"))]
    let _ = elapsed;

    match result {
        Ok(Some(path)) => {
            #[cfg(target_os = "windows")]
            remember_vct_trace_line(format!(
                "[quintet-vct] engine=dfpns potential_min=3 attacker_vcf_depth={} result=found len={} ms={}",
                QUINTET_THREAT_VCF_DEPTH,
                path.len(),
                elapsed,
            ));
            SolveOutcome::Found(path)
        }
        Ok(None) => {
            #[cfg(target_os = "windows")]
            remember_vct_trace_line(format!(
                "[quintet-vct] engine=dfpns potential_min=3 attacker_vcf_depth={} result=not_found ms={}",
                QUINTET_THREAT_VCF_DEPTH,
                elapsed,
            ));
            SolveOutcome::NotFound
        }
        Err(outcome) => outcome,
    }
}

pub(super) fn solve(
    board: [u8; BOARD_CELLS],
    turn: u8,
    max_depth: usize,
    time_limit: Duration,
    cancel: Arc<AtomicBool>,
    _requested_mode: SearchMode,
) -> SolveOutcome {
    solve_internal(board, turn, max_depth, time_limit, cancel, None)
}

pub(super) fn solve_with_progress<F>(
    board: [u8; BOARD_CELLS],
    turn: u8,
    max_depth: usize,
    time_limit: Duration,
    cancel: Arc<AtomicBool>,
    _requested_mode: SearchMode,
    progress: F,
) -> SolveOutcome
where
    F: FnMut(Vec<Move>) + Send + 'static,
{
    solve_internal(
        board,
        turn,
        max_depth,
        time_limit,
        cancel,
        Some(Box::new(progress)),
    )
}
