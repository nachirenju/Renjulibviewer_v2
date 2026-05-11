// Zobristハッシュ計算と対称性を考慮した、軽量な盤面状態管理を行う
use super::zobrist::Zobrist;

/// ハッシュ計算専用の盤面表現
/// UI用の `board::Board` とは異なり、Zobristハッシュの高速計算に特化した構造体
#[derive(Clone)]
pub struct Board {
    pub occupied: [u64; 4],
    pub hashes: [u64; 8], 
    pub move_count: usize,
}

impl Board {
    pub fn new() -> Self {
        Self { occupied: [0; 4], hashes: [0; 8], move_count: 0 }
    }

    pub fn get_canonical_hash(&self) -> u64 {
        *self.hashes.iter().min().unwrap()
    }

    #[inline(always)]
    fn update_hashes(&mut self, x: i8, y: i8, color_idx: usize) {
        let t_table = Zobrist::transformed_table();
        unsafe {
            let cell_hashes = t_table.get_unchecked((y as usize * 15) + x as usize);
            for t in 0..8 {
                *self.hashes.get_unchecked_mut(t) ^= *cell_hashes.get_unchecked(t).get_unchecked(color_idx);
            }
        }
    }

    #[inline(always)]
    pub fn make_move(&mut self, x: i8, y: i8) {
        if x >= 0 && y >= 0 && x < 15 && y < 15 {
            let idx = (y as usize * 15) + x as usize;
            let color_idx = self.move_count % 2;
            unsafe { *self.occupied.get_unchecked_mut(idx / 64) |= 1 << (idx % 64); }
            self.update_hashes(x, y, color_idx);
        }
        self.move_count += 1;
    }

    #[inline(always)]
    pub fn make_move_with_color(&mut self, x: i8, y: i8, color_idx: usize) {
        if x >= 0 && y >= 0 && x < 15 && y < 15 {
            let idx = (y as usize * 15) + x as usize;
            unsafe { *self.occupied.get_unchecked_mut(idx / 64) |= 1 << (idx % 64); }
            self.update_hashes(x, y, color_idx);
        }
        self.move_count += 1;
    }

    #[inline(always)]
    pub fn undo_move(&mut self, x: i8, y: i8) {
        if self.move_count > 0 { self.move_count -= 1; }
        if x >= 0 && y >= 0 && x < 15 && y < 15 {
            let idx = (y as usize * 15) + x as usize;
            let color_idx = self.move_count % 2;
            unsafe { *self.occupied.get_unchecked_mut(idx / 64) &= !(1 << (idx % 64)); }
            self.update_hashes(x, y, color_idx);
        }
    }
}
