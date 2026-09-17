use crate::board::Direction::*;
use crate::board::Player::*;
use crate::board::*;

#[derive(Default, Clone)]
pub struct Potential {
    v: u8,
    h: u8,
    a: u8,
    d: u8,
}

impl Potential {
    fn set(&mut self, d: Direction, o: u8) {
        match d {
            Vertical => self.v = o,
            Horizontal => self.h = o,
            Ascending => self.a = o,
            Descending => self.d = o,
        }
    }

    fn sum(&self) -> u8 {
        // TODO: faster computation
        self.v + self.h + self.a + self.d
    }

    fn direction_count(&self) -> u8 {
        [self.v, self.h, self.a, self.d]
            .iter()
            .filter(|&&value| value > 0)
            .count() as u8
    }
}

type Potentials = [[Potential; RANGE as usize]; RANGE as usize];
type PotentialBuckets = [[u16; RANGE as usize]; (u8::MAX as usize) + 1];
type DirectionBuckets = [[u16; RANGE as usize]; 5];
type ScoreDirectionCounts = [[u16; 5]; (u8::MAX as usize) + 1];

const BOARD_BITS: u16 = (1u16 << RANGE) - 1;

#[derive(Clone)]
pub struct PotentialField {
    potentials: Potentials,
    buckets: PotentialBuckets,
    direction_buckets: DirectionBuckets,
    bucket_counts: [u16; (u8::MAX as usize) + 1],
    score_direction_counts: ScoreDirectionCounts,
    max_score: u8,
    player: Player,
    min: u8,
}

impl PotentialField {
    pub fn new(player: Player, min: u8) -> Self {
        let mut buckets = [[0u16; RANGE as usize]; (u8::MAX as usize) + 1];
        buckets[0].fill(BOARD_BITS);
        let mut direction_buckets = [[0u16; RANGE as usize]; 5];
        direction_buckets[0].fill(BOARD_BITS);
        let mut bucket_counts = [0u16; (u8::MAX as usize) + 1];
        bucket_counts[0] = (RANGE as u16) * (RANGE as u16);
        let mut score_direction_counts = [[0u16; 5]; (u8::MAX as usize) + 1];
        score_direction_counts[0][0] = (RANGE as u16) * (RANGE as u16);
        Self {
            potentials: Potentials::default(),
            buckets,
            direction_buckets,
            bucket_counts,
            score_direction_counts,
            max_score: 0,
            player: player,
            min: min,
        }
    }

    pub fn init(player: Player, min: u8, board: &Board) -> Self {
        let mut result = Self::new(player, min);
        let os = board.potentials(player, min, player.is_black());
        for (idx, o) in os {
            result.set(idx, o);
        }
        result
    }

    pub fn update_along(&mut self, p: Point, board: &Board) {
        self.reset_along(p);
        let os = board.potentials_along(p, self.player, self.min, self.player.is_black());
        for (idx, o) in os {
            self.set(idx, o);
        }
    }

    pub fn get(&self, p: Point) -> u8 {
        self.sum(p)
    }

    pub fn direction_count(&self, p: Point) -> u8 {
        self.potentials[p.0 as usize][p.1 as usize].direction_count()
    }

    pub fn collect(&self, min: u8) -> Vec<(Point, u8)> {
        (0..RANGE)
            .flat_map(|x| {
                (0..RANGE).map(move |y| {
                    let p = Point(x, y);
                    (p, self.sum(p))
                })
            })
            .filter(|&(_, o)| o >= min)
            .collect()
    }

    /// Collects matching points in the same order as a stable descending
    /// potential sort with a direction-count tie-break, without scanning all
    /// board cells or sorting the result.
    pub fn collect_sorted(&self, min: u8) -> Vec<(Point, u8)> {
        let capacity = self.count_at_least(min);
        let mut result = Vec::with_capacity(capacity);
        result.extend(self.iter_sorted(min));
        result
    }

    /// Iterates matching points directly from the score buckets. The order is
    /// score descending, connected-direction count descending, then x/y
    /// ascending for exact ties.
    pub fn iter_sorted(&self, min: u8) -> impl Iterator<Item = (Point, u8)> + '_ {
        let max_score = self.max_score;
        (min..=max_score)
            .rev()
            .filter(move |&score| self.bucket_counts[score as usize] > 0)
            .flat_map(move |score| {
                (0..=4u8)
                    .rev()
                    .filter(move |&directions| {
                        self.score_direction_counts[score as usize][directions as usize] > 0
                    })
                    .flat_map(move |directions| {
                        (0..RANGE).flat_map(move |x| {
                            let mut ys = self.buckets[score as usize][x as usize]
                                & self.direction_buckets[directions as usize][x as usize];
                            std::iter::from_fn(move || {
                                if ys == 0 {
                                    return None;
                                }
                                let y = ys.trailing_zeros() as u8;
                                ys &= ys - 1;
                                Some((Point(x, y), score))
                            })
                        })
                    })
            })
    }

    pub fn count_at_least(&self, min: u8) -> usize {
        if min > self.max_score {
            return 0;
        }
        self.bucket_counts[min as usize..=self.max_score as usize]
            .iter()
            .map(|&count| count as usize)
            .sum()
    }

    #[allow(dead_code)]
    pub fn overlay(&self, board: &Board) -> String {
        (0..RANGE)
            .rev()
            .map(|y| {
                (0..RANGE)
                    .map(|x| {
                        let p = Point(x, y);
                        match board.stone(p) {
                            Some(Black) => " o".to_string(),
                            Some(White) => " x".to_string(),
                            None => match self.sum(p) {
                                0 => " .".to_string(),
                                po => format!("{: >2}", po),
                            },
                        }
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn reset_along(&mut self, p: Point) {
        let indices = [
            p.to_index(Vertical),
            p.to_index(Horizontal),
            p.to_index(Ascending),
            p.to_index(Descending),
        ];
        let neighbor_indices = indices.iter().flat_map(|idx| {
            (0..RANGE).flat_map(move |j| {
                if j <= idx.maxj() {
                    Some(Index::new(idx.d, idx.i, j))
                } else {
                    None
                }
            })
        });
        for idx in neighbor_indices {
            self.set(idx, 0);
        }
    }

    fn sum(&self, p: Point) -> u8 {
        self.potentials[p.0 as usize][p.1 as usize].sum()
    }

    fn set(&mut self, i: Index, o: u8) {
        let p = i.to_point();
        let old_score = self.sum(p);
        let old_directions = self.direction_count(p);
        self.potentials[p.0 as usize][p.1 as usize].set(i.d, o);
        let new_score = self.sum(p);
        let new_directions = self.direction_count(p);
        if old_score == new_score && old_directions == new_directions {
            return;
        }

        let bit = 1u16 << p.1;
        self.score_direction_counts[old_score as usize][old_directions as usize] -= 1;
        self.score_direction_counts[new_score as usize][new_directions as usize] += 1;
        if old_score != new_score {
            self.buckets[old_score as usize][p.0 as usize] &= !bit;
            self.buckets[new_score as usize][p.0 as usize] |= bit;
            self.bucket_counts[old_score as usize] -= 1;
            self.bucket_counts[new_score as usize] += 1;
            if new_score > self.max_score {
                self.max_score = new_score;
            } else if old_score == self.max_score && self.bucket_counts[old_score as usize] == 0 {
                while self.max_score > 0 && self.bucket_counts[self.max_score as usize] == 0 {
                    self.max_score -= 1;
                }
            }
        }
        if old_directions != new_directions {
            self.direction_buckets[old_directions as usize][p.0 as usize] &= !bit;
            self.direction_buckets[new_directions as usize][p.0 as usize] |= bit;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_sorted_matches_scan(field: &PotentialField, min: u8) {
        let mut expected = field.collect(min);
        expected.sort_by(|&a, &b| {
            b.1.cmp(&a.1)
                .then_with(|| field.direction_count(b.0).cmp(&field.direction_count(a.0)))
        });
        assert_eq!(field.collect_sorted(min), expected);
    }

    #[test]
    fn equal_potential_prefers_more_connected_directions() {
        let mut field = PotentialField::new(Black, 2);
        let single_direction = Point(3, 3);
        let connected = Point(9, 9);
        field.set(single_direction.to_index(Horizontal), 4);
        field.set(connected.to_index(Horizontal), 2);
        field.set(connected.to_index(Vertical), 2);

        assert_eq!(field.get(single_direction), field.get(connected));
        assert_eq!(field.direction_count(single_direction), 1);
        assert_eq!(field.direction_count(connected), 2);
        assert_eq!(field.collect_sorted(3)[0].0, connected);
    }

    #[test]
    fn sorted_buckets_match_full_scan_after_updates_and_undo() {
        let mut board = Board::new();
        let mut field = PotentialField::init(Black, 2, &board);
        assert_sorted_matches_scan(&field, 2);
        assert_sorted_matches_scan(&field, 3);

        let moves = [
            (Black, Point(7, 7)),
            (White, Point(7, 8)),
            (Black, Point(8, 7)),
            (White, Point(6, 8)),
            (Black, Point(9, 7)),
            (White, Point(8, 8)),
            (Black, Point(6, 7)),
        ];
        for &(player, point) in &moves {
            board.put_mut(player, point);
            field.update_along(point, &board);
            assert_sorted_matches_scan(&field, 2);
            assert_sorted_matches_scan(&field, 3);
        }

        for &(_, point) in moves.iter().rev() {
            board.remove_mut(point);
            field.update_along(point, &board);
            assert_sorted_matches_scan(&field, 2);
            assert_sorted_matches_scan(&field, 3);
        }
    }

    #[test]
    fn test_overlay() -> Result<(), String> {
        let board = "
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . o . . o . . . . .
             . . . . . . o x x . . . . . .
             . . . . . . . o . . . . . . .
             . . . . . . . . x . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
        "
        .parse::<Board>()?;

        let field = PotentialField::init(Black, 3, &board);
        let result = field.overlay(&board);
        let expected = trim_lines_string(
            "
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . 3 . . . . . . . .
             . . . 3 . . 6 . . . . . . . .
             . . . . 3 . 9 . . . . . . . .
             . . . . . 6 o 6 6 o 3 . . . .
             . . . . . . o x x . . . . . .
             . . . . . . 9 o . . . . . . .
             . . . . . . 6 . x . . . . . .
             . . . . . . 3 . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
        ",
        );
        assert_eq!(result, expected);

        let field = PotentialField::init(Black, 2, &board);
        let result = field.overlay(&board);
        let expected = trim_lines_string(
            "
             . . . . . . . . . . . . . . .
             . . 2 . . 2 . . . 2 2 . . 2 .
             . . . 2 . . 7 . . 8 2 . 2 . .
             . . . 3 2 . 6 6 610 . 2 . . .
             . . . . 3 2 9 814 8 2 . . . .
             . . 2 4 4 6 o14 6 o 3 4 4 2 .
             . . 2 2 210 o x x 8 8 . . . .
             . . . 2101417 o 812 4 8 . . .
             . . . 4 6 . 8 2 x 4 . . 4 . .
             . . 2 4 . 2 3 2 . 2 . . . 2 .
             . . 2 . 2 . . 2 . . . . . . .
             . . . 2 . . . 2 . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
            ",
        );
        assert_eq!(result, expected);

        let field = PotentialField::init(White, 3, &board);
        let result = field.overlay(&board);
        let expected = trim_lines_string(
            "
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . 3 . . . . . .
             . . . . . . o . 6 o . . . . .
             . . . . . . o x x 3 3 3 . . .
             . . . . . . . o 9 . . . . . .
             . . . . . . . . x . . . . . .
             . . . . . . . . 6 . . . . . .
             . . . . . . . . 3 . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
        ",
        );
        assert_eq!(result, expected);

        let field = PotentialField::init(White, 2, &board);
        let result = field.overlay(&board);
        let expected = trim_lines_string(
            "
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . 2 . . 2 2 . . 2 . . .
             . . . . . 4 . 2 4 . 4 . . . .
             . . . . . . 6 2 3 6 . . 2 . .
             . . . . . . o1014 o . 4 . . .
             . . . . . . o x x 3 9 3 2 . .
             . . . . . . 8 o1116 . . . . .
             . . . . 210 6 8 x1012 4 2 . .
             . . . . 4 . . 8 6 2 2 4 . . .
             . . . 2 . . 6 . 3 . 2 2 2 . .
             . . . . . 4 . . 4 . . 2 . . .
             . . . . 2 . . . 2 . . . 2 . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
        ",
        );
        assert_eq!(result, expected);

        Ok(())
    }

    #[test]
    fn test_collect() -> Result<(), String> {
        let mut board = "
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . o . x o . . . . .
             . . . . . . o x x . . . . . .
             . . . . . . . o o . . . . . .
             . . . . . . . . x . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
        "
        .parse::<Board>()?;

        let mut field = PotentialField::init(White, 3, &board);
        let result = field.overlay(&board);
        let expected = trim_lines_string(
            "
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . 3 . . 3 . . .
             . . . . . . . . 3 . 6 . . . .
             . . . . . . . . 3 9 . . . . .
             . . . . . . o . x o . . . . .
             . . . . . . o x x 3 3 3 . . .
             . . . . . . 9 o o . . . . . .
             . . . . . 6 . . x . . . . . .
             . . . . 3 . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
            ",
        );
        assert_eq!(result, expected);

        let p = Point(5, 6);
        board.put_mut(White, p);
        field.update_along(p, &board);
        let result = field.overlay(&board);
        let expected = trim_lines_string(
            "
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . 3 . . 3 . . .
             . . . . . . . . 3 . 6 . . . .
             . . . . . . . . 3 4 . . . . .
             . . . . . . o . x o . . . . .
             . . . . . . o x x 3 3 3 . . .
             . . . . . . 8 o o . . . . . .
             . . . . 3 x 6 6 x 3 . . . . .
             . . . . 4 . . . . . . . . . .
             . . . 3 . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
             . . . . . . . . . . . . . . .
            ",
        );
        assert_eq!(result, expected);

        let result = field.collect(3);
        let expected = [
            (Point(3, 4), 3),
            (Point(4, 5), 4),
            (Point(4, 6), 3),
            (Point(6, 6), 6),
            (Point(6, 7), 8),
            (Point(7, 6), 6),
            (Point(8, 10), 3),
            (Point(8, 11), 3),
            (Point(8, 12), 3),
            (Point(9, 6), 3),
            (Point(9, 8), 3),
            (Point(9, 10), 4),
            (Point(10, 8), 3),
            (Point(10, 11), 6),
            (Point(11, 8), 3),
            (Point(11, 12), 3),
        ];
        assert_eq!(result, expected);

        Ok(())
    }

    fn trim_lines_string(s: &str) -> String {
        s.trim()
            .split("\n")
            .map(|ls| " ".to_string() + ls.trim())
            .collect::<Vec<_>>()
            .join("\n")
    }
}
