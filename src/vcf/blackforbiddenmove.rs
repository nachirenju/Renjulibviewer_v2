use super::model::{BOARD_CELLS, BOARD_SIZE};

pub const EMPTY: u8 = 0;
pub const BLACK: u8 = 1;
pub const WHITE: u8 = 2;

const DIRS: [(isize, isize); 4] = [(1, 0), (0, 1), (1, 1), (1, -1)];
const MAX_FORBIDDEN_RECURSION: usize = BOARD_CELLS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlackForbiddenKind {
    Overline,
    DoubleFour,
    DoubleThree,
}

impl BlackForbiddenKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Overline => "overline",
            Self::DoubleFour => "double-four",
            Self::DoubleThree => "double-three",
        }
    }
}

pub fn is_forbidden(board: [u8; BOARD_CELLS], idx: usize) -> Option<BlackForbiddenKind> {
    if idx >= BOARD_CELLS || board[idx] != EMPTY {
        return None;
    }

    let mut checker = BlackForbiddenMove::new(board);
    checker.board[idx] = BLACK;
    if checker.check_win(idx % BOARD_SIZE, idx / BOARD_SIZE) {
        return None;
    }
    checker.forbidden(idx, true)
}

pub fn placed_forbidden(
    board: [u8; BOARD_CELLS],
    idx: usize,
    check_three: bool,
) -> Option<BlackForbiddenKind> {
    if idx >= BOARD_CELLS || board[idx] != BLACK {
        return None;
    }
    BlackForbiddenMove::new(board).forbidden(idx, check_three)
}

pub fn forbidden_points(board: [u8; BOARD_CELLS]) -> Vec<usize> {
    let mut points = Vec::new();
    for idx in 0..BOARD_CELLS {
        if is_forbidden(board, idx).is_some() {
            points.push(idx);
        }
    }
    points
}

struct BlackForbiddenMove {
    board: [u8; BOARD_CELLS],
}

impl BlackForbiddenMove {
    fn new(board: [u8; BOARD_CELLS]) -> Self {
        Self { board }
    }

    fn forbidden(&mut self, idx: usize, check_three: bool) -> Option<BlackForbiddenKind> {
        self.forbidden_at_depth(idx, check_three, 0)
    }

    fn forbidden_at_depth(
        &mut self,
        idx: usize,
        check_three: bool,
        depth: usize,
    ) -> Option<BlackForbiddenKind> {
        let (x, y) = xy(idx);

        // 1. 長連を最初に調べる。黒は六連以上になった時点で禁手。
        for (dx, dy) in DIRS {
            if self.line_count(x, y, dx, dy) >= 6 {
                return Some(BlackForbiddenKind::Overline);
            }
        }

        // 2. 四の数を数える。同一方向の四四を重複して数えないよう、実質的な四を集計する。
        let mut fours = 0;
        for (dx, dy) in DIRS {
            let count = self.line_count(x, y, dx, dy);
            let mut patterns = self.four_patterns(x, y, dx, dy);
            if count == 4 && patterns > 1 {
                patterns = 1;
            }
            fours += patterns;
        }
        if fours >= 2 {
            return Some(BlackForbiddenKind::DoubleFour);
        }

        if !check_three {
            return None;
        }
        if depth >= MAX_FORBIDDEN_RECURSION {
            return Some(BlackForbiddenKind::DoubleThree);
        }

        // 3. 三を数える。達四点に黒を置いて、合法な活四になるものだけを「三」とする。
        //    達四点そのものが三々禁などの禁手なら、その三は成立しない。
        let mut threes = 0;
        for (dx, dy) in DIRS {
            for vital in self.three_vital_points(x, y, dx, dy) {
                self.board[vital] = BLACK;
                let valid = self.forbidden_at_depth(vital, true, depth + 1).is_none()
                    && self.valid_straight_four(vital);
                self.board[vital] = EMPTY;

                if valid {
                    threes += 1;
                    break;
                }
            }
        }

        (threes >= 2).then_some(BlackForbiddenKind::DoubleThree)
    }

    fn four_patterns(&mut self, x: usize, y: usize, dx: isize, dy: isize) -> usize {
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
                match self.board[idx] {
                    BLACK => stones += 1,
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

                let (gx, gy) = xy(gap);
                self.board[gap] = BLACK;
                let count = self.line_count(gx, gy, dx, dy);
                self.board[gap] = EMPTY;
                if count <= 5 {
                    spots.push(gap);
                }
            }
        }
        spots.len()
    }

    fn three_vital_points(&self, x: usize, y: usize, dx: isize, dy: isize) -> Vec<usize> {
        let mut line = [WHITE; 11];
        for k in -5..=5 {
            line[(k + 5) as usize] = if k == 0 {
                BLACK
            } else {
                index(x as isize + k * dx, y as isize + k * dy)
                    .map(|idx| match self.board[idx] {
                        BLACK => BLACK,
                        EMPTY => EMPTY,
                        _ => WHITE,
                    })
                    .unwrap_or(WHITE)
            };
        }

        // 4. 連珠の「三」は、どちらかの達四点に打つと活四へ進める形だけを候補にする。
        let mut points = Vec::new();
        for start in 1..=6 {
            let window = &line[start..start + 5];
            if window.iter().filter(|&&v| v == BLACK).count() != 3
                || window.iter().filter(|&&v| v == EMPTY).count() != 2
                || line[start - 1] == BLACK
                || (start + 5 < 11 && line[start + 5] == BLACK)
            {
                continue;
            }
            for offset in 0..5 {
                if window[offset] == EMPTY
                    && ((offset > 0 && window[offset - 1] == BLACK)
                        || (offset + 1 < 5 && window[offset + 1] == BLACK))
                {
                    let k = (start + offset) as isize - 5;
                    if let Some(idx) = index(x as isize + k * dx, y as isize + k * dy)
                        && !points.contains(&idx)
                    {
                        points.push(idx);
                    }
                }
            }
        }
        points
    }

    fn valid_straight_four(&mut self, idx: usize) -> bool {
        let (x, y) = xy(idx);
        for (dx, dy) in DIRS {
            for start in -4..=0 {
                if !self.is_value(x, y, dx, dy, start, EMPTY)
                    || !self.is_value(x, y, dx, dy, start + 5, EMPTY)
                    || (1..=4).any(|k| !self.is_value(x, y, dx, dy, start + k, BLACK))
                {
                    continue;
                }
                if self.can_win_at(x, y, dx, dy, start)
                    && self.can_win_at(x, y, dx, dy, start + 5)
                {
                    return true;
                }
            }
        }
        false
    }

    fn check_win(&self, x: usize, y: usize) -> bool {
        DIRS.into_iter()
            .any(|(dx, dy)| self.line_count(x, y, dx, dy) == 5)
    }

    fn line_count(&self, x: usize, y: usize, dx: isize, dy: isize) -> usize {
        let mut count = 1;
        for sign in [-1, 1] {
            let mut step = 1;
            while let Some(idx) =
                index(x as isize + dx * step * sign, y as isize + dy * step * sign)
            {
                if self.board[idx] != BLACK {
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
            .is_some_and(|idx| self.board[idx] == expected)
    }

    fn can_win_at(&mut self, x: usize, y: usize, dx: isize, dy: isize, k: isize) -> bool {
        let Some(idx) = index(x as isize + k * dx, y as isize + k * dy) else {
            return false;
        };
        if self.board[idx] != EMPTY {
            return false;
        }
        let (tx, ty) = xy(idx);
        self.board[idx] = BLACK;
        let wins = self.check_win(tx, ty);
        self.board[idx] = EMPTY;
        wins
    }
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
