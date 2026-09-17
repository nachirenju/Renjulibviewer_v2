use crate::analysis::field::PotentialField;
use crate::board::StructureKind::*;
use crate::board::*;
use crate::mate::game::*;
use crate::mate::mate::Mate;
use crate::mate::state::State;
use crate::mate::vcf::VCFState;

pub struct VCTState {
    game: Game,
    pub attacker: Player,
    pub limit: u8,
    field: PotentialField,
}

impl VCTState {
    pub fn new(game: Game, limit: u8, field: PotentialField) -> Self {
        Self {
            attacker: game.turn,
            game: game,
            limit: limit,
            field: field,
        }
    }

    pub fn init(board: &Board, attacker: Player, limit: u8) -> Self {
        let game = Game::init(board, attacker);
        let field = PotentialField::init(attacker, 2, board);
        Self::new(game, limit, field)
    }

    pub fn vcf_state(&self, max_limit: u8) -> VCFState {
        let game = self.game.clone();
        let limit = self.limit.min(max_limit);
        VCFState::new(game, limit)
    }

    pub fn threat_state(&self, max_limit: u8) -> VCFState {
        let mut game = self.game.clone();
        game.play(None);
        let limit = if self.attacking() {
            self.limit.saturating_sub(1)
        } else {
            self.limit
        }
        .min(max_limit);
        VCFState::new(game, limit)
    }

    pub fn is_forbidden_move(&self, p: Point) -> bool {
        self.game().is_forbidden_move(p)
    }

    pub fn check_event(&self) -> Option<Event> {
        self.game().check_event()
    }

    pub fn next_zobrist_hash(&self, next_move: Option<Point>) -> u64 {
        let next_limit = if !self.attacking() {
            self.limit.saturating_sub(1)
        } else {
            self.limit
        };
        match next_move {
            Some(point) => {
                self.game
                    .board()
                    .zobrist_hash_after_put_n(self.game.turn, point, next_limit)
            }
            None => self.game.board().zobrist_hash_n(next_limit),
        }
    }

    pub fn sorted_potential_iter(&self, min: u8) -> impl Iterator<Item = (Point, u8)> + '_ {
        self.field.iter_sorted(min)
    }

    pub fn sorted_potential_count(&self, min: u8) -> usize {
        self.field.count_at_least(min)
    }

    #[allow(dead_code)]
    pub fn sorted_potentials(&self, min: u8, only: Option<Vec<Point>>) -> Vec<(Point, u8)> {
        self.sorted_potentials_counted(min, only).0
    }

    pub fn sorted_potentials_counted(
        &self,
        min: u8,
        only: Option<Vec<Point>>,
    ) -> (Vec<(Point, u8)>, usize) {
        if let Some(only) = only {
            let scored = only
                .into_iter()
                .map(|p| (p, self.field.get(p)))
                .filter(|&(_, o)| o >= min)
                .collect();
            let (mut result, before_dedup) = dedup_scored_points(scored);
            result.sort_by(|&a, &b| {
                b.1.cmp(&a.1).then_with(|| {
                    self.field
                        .direction_count(b.0)
                        .cmp(&self.field.direction_count(a.0))
                })
            });
            (result, before_dedup)
        } else {
            let result = self.field.collect_sorted(min);
            let before_dedup = result.len();
            (result, before_dedup)
        }
    }

    pub fn sort_by_potential(&self, points: Vec<Point>) -> Vec<(Point, u8)> {
        self.sort_by_potential_counted(points).0
    }

    pub fn sort_by_potential_counted(&self, points: Vec<Point>) -> (Vec<(Point, u8)>, usize) {
        let (points, before_dedup) = dedup_points(points);
        let mut result: Vec<_> = points.into_iter().map(|p| (p, self.field.get(p))).collect();
        result.sort_by(|&a, &b| b.1.cmp(&a.1));
        (result, before_dedup)
    }

    pub fn empties(&self) -> Vec<Point> {
        self.game().board().empties().collect()
    }

    /// The currently descended VCT path, optionally extended with the next
    /// most-proving move selected by DF-PNS.
    pub fn progress_path(&self, next: Option<Point>) -> Vec<Point> {
        let mut path = self
            .game()
            .moves()
            .iter()
            .filter_map(|point| *point)
            .collect::<Vec<_>>();
        if let Some(next) = next {
            path.push(next);
        }
        path
    }

    pub fn threat_defences(&self, threat: &Mate) -> Vec<Point> {
        let mut result = threat.path().clone();
        result.extend(self.end_breakers(threat.end().clone()));
        result.extend(self.counter_defences(threat));
        result.extend(self.four_moves());
        result
    }

    pub fn end_breakers(&self, end: End) -> Vec<Point> {
        match end {
            Fours(p1, p2) => {
                vec![p1, p2]
            }
            Forbidden(p) => {
                let mut ds = vec![p];
                ds.extend(self.game().board().neighbors(p, 5, true));
                ds
            }
            _ => vec![],
        }
    }

    pub fn four_moves(&self) -> Vec<Point> {
        self.game()
            .board()
            .structures(self.game().turn, Sword)
            .flat_map(|s| s.eyes())
            .collect()
    }

    fn counter_defences(&self, threat: &Mate) -> Vec<Point> {
        let mut game = self.game().clone();
        game.play(None);
        let threater = game.turn;
        let mut result = vec![];
        for &p in &threat.path {
            let turn = game.turn;
            game.play(Some(p));
            if turn == threater {
                continue;
            }
            let swords = game.board().structures_on(p, turn, Sword);
            for s in swords {
                result.extend(s.eyes());
            }
        }
        result
    }
}

fn dedup_points(points: Vec<Point>) -> (Vec<Point>, usize) {
    let before_dedup = points.len();
    let mut seen = [false; (RANGE as usize) * (RANGE as usize)];
    let mut unique = Vec::with_capacity(before_dedup);
    for point in points {
        let index = point.1 as usize * RANGE as usize + point.0 as usize;
        if !seen[index] {
            seen[index] = true;
            unique.push(point);
        }
    }
    (unique, before_dedup)
}

fn dedup_scored_points(points: Vec<(Point, u8)>) -> (Vec<(Point, u8)>, usize) {
    let before_dedup = points.len();
    let mut seen = [false; (RANGE as usize) * (RANGE as usize)];
    let mut unique = Vec::with_capacity(before_dedup);
    for (point, potential) in points {
        let index = point.1 as usize * RANGE as usize + point.0 as usize;
        if !seen[index] {
            seen[index] = true;
            unique.push((point, potential));
        }
    }
    (unique, before_dedup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn potential_ordering_deduplicates_points_before_sorting() {
        let board = Board::new();
        let state = VCTState::init(&board, crate::board::Player::Black, 4);
        let first = Point(3, 4);
        let second = Point(8, 9);

        let (ordered, before_dedup) =
            state.sort_by_potential_counted(vec![first, second, first, second, first]);

        assert_eq!(before_dedup, 5);
        assert_eq!(ordered.len(), 2);
        assert_eq!(ordered[0].0, first);
        assert_eq!(ordered[1].0, second);
    }

    #[test]
    fn direct_next_hash_matches_play_and_undo_hash() {
        let mut board = Board::new();
        board.put_mut(crate::board::Player::Black, Point(7, 7));
        board.put_mut(crate::board::Player::White, Point(8, 8));

        for &(attacker, limit, point) in &[
            (crate::board::Player::Black, 30, Point(6, 7)),
            (crate::board::Player::White, 17, Point(9, 8)),
        ] {
            let mut state = VCTState::init(&board, attacker, limit);
            for next_move in [Some(point), None] {
                let expected_limit = if !state.attacking() {
                    state.limit.saturating_sub(1)
                } else {
                    state.limit
                };
                let expected = state
                    .game
                    .into_play(next_move, |game| game.zobrist_hash(expected_limit));
                assert_eq!(state.next_zobrist_hash(next_move), expected);
            }

            state.play(Some(point));
            let defence = Point(point.0.saturating_add(1).min(RANGE - 1), point.1);
            let expected_limit = state.limit.saturating_sub(1);
            let expected = state
                .game
                .into_play(Some(defence), |game| game.zobrist_hash(expected_limit));
            assert_eq!(state.next_zobrist_hash(Some(defence)), expected);
        }
    }
}

impl State for VCTState {
    fn game(&self) -> &Game {
        &self.game
    }

    fn game_mut(&mut self) -> &mut Game {
        &mut self.game
    }

    fn attacker(&self) -> Player {
        self.attacker
    }

    fn limit(&self) -> u8 {
        self.limit
    }

    fn set_limit(&mut self, limit: u8) {
        self.limit = limit
    }

    fn after_play(&mut self, next_move: Option<Point>) {
        if let Some(next_move) = next_move {
            self.field.update_along(next_move, self.game.board());
        }
    }

    fn after_undo(&mut self, maybe_last_move: Option<Point>) {
        if let Some(last_move) = maybe_last_move {
            self.field.update_along(last_move, self.game.board());
        }
    }
}
