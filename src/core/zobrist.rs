// Zobristハッシュの定数テーブルおよび対称変換テーブルの管理を行う
use std::sync::OnceLock;

#[derive(Clone)]
pub struct Zobrist;

impl Zobrist {
    pub fn table() -> &'static [[u64; 2]; 225] {
        static TABLE: OnceLock<[[u64; 2]; 225]> = OnceLock::new();
        TABLE.get_or_init(|| {
            let mut t = [[0u64; 2]; 225];
            let mut val = 0x123456789ABCDEF0u64;
            for i in 0..225 {
                for j in 0..2 {
                    val = val.wrapping_mul(6364136223846793005).wrapping_add(1);
                    t[i][j] = val;
                }
            }
            t
        })
    }

    #[inline(always)]
    pub fn get_transformed_idx(x: i8, y: i8, t: usize) -> usize {
        if x < 0 || y < 0 || x > 14 || y > 14 { return 0; }
        let cx = 7; 
        let cy = 7;
        let (tx, ty) = match t {
            0 => (x, y),                                   
            1 => (cx + (y - cy), cy - (x - cx)),           
            2 => (cx - (x - cx), cy - (y - cy)),           
            3 => (cx - (y - cy), cy + (x - cx)),           
            4 => (x, cy - (y - cy)),                       
            5 => (cx + (y - cy), cy + (x - cx)),           
            6 => (cx - (x - cx), cy + (y - cy)),           
            7 => (cx - (y - cy), cy - (x - cx)),           
            _ => unreachable!(),
        };
        (ty as usize * 15) + tx as usize
    }

    pub fn compose_t(t1: usize, t2: usize) -> usize {
        let pt1 = Self::get_transformed_idx(1, 2, t1);
        let pt2 = Self::get_transformed_idx((pt1 % 15) as i8, (pt1 / 15) as i8, t2);
        for t in 0..8 {
            if Self::get_transformed_idx(1, 2, t) == pt2 {
                return t;
            }
        }
        0
    }

    pub fn transformed_table() -> &'static [[[u64; 2]; 8]; 225] {
        static TRANS_TABLE: OnceLock<[[[u64; 2]; 8]; 225]> = OnceLock::new();
        TRANS_TABLE.get_or_init(|| {
            let base_table = Self::table();
            let mut t_table = [[[0u64; 2]; 8]; 225];
            for y in 0..15 {
                for x in 0..15 {
                    let idx = y * 15 + x;
                    for t in 0..8 {
                        let t_idx = Self::get_transformed_idx(x as i8, y as i8, t);
                        t_table[idx][t][0] = base_table[t_idx][0];
                        t_table[idx][t][1] = base_table[t_idx][1];
                    }
                }
            }
            t_table
        })
    }
}
