/// 着手履歴から標準的な座標表記（例: h8h9j7）を生成する
pub fn generate_notation(history: &[usize]) -> String {
    let mut s = String::new();
    for &m in history {
        let x = m % 15;
        let y = m / 15;
        let col = (b'a' + x as u8) as char;
        let row = 15 - y;
        s.push_str(&format!("{}{}", col, row));
    }
    s
}

/// 着手表記から座標のリストを抽出する
pub fn parse_notation(text: &str) -> Vec<(usize, usize)> {
    let mut moves = Vec::new();
    let text = text.to_lowercase();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c >= 'a' && c <= 'o' {
            let x = (c as u8 - b'a') as usize;
            i += 1;
            let mut row_str = String::new();
            while i < chars.len() && chars[i].is_digit(10) {
                row_str.push(chars[i]);
                i += 1;
            }
            if let Ok(row) = row_str.parse::<usize>() {
                if row >= 1 && row <= 15 {
                    let y = 15 - row;
                    moves.push((x, y));
                }
            }
        } else {
            i += 1;
        }
    }
    moves
}

/// 着手履歴からSGF形式の文字列を生成する
pub fn generate_sgf(history: &[usize]) -> String {
    let mut s = String::from("(;GM[1]SZ[15]");
    if !history.is_empty() {
        s.push('\n');
    }
    for (i, &m) in history.iter().enumerate() {
        let x = m % 15;
        let y = m / 15;
        let col = (b'a' + x as u8) as char;
        let row = (b'a' + y as u8) as char;
        let color = if i % 2 == 0 { "B" } else { "W" };
        s.push_str(&format!(";{}[{}{}]", color, col, row));
    }
    s.push(')');
    s
}

/// SGF形式のテキストを解析し、着手座標のリストを返す
pub fn parse_sgf(text: &str) -> Vec<(usize, usize)> {
    let mut moves = Vec::new();
    let text = text.to_lowercase();
    let mut i = 0;
    while let Some(idx) = text[i..].find('[') {
        let start = i + idx + 1;
        if start + 1 < text.len() && text[start..].contains(']') {
            let col_char = text.as_bytes()[start];
            let row_char = text.as_bytes()[start + 1];
            if col_char >= b'a' && col_char <= b'o' && row_char >= b'a' && row_char <= b'o' {
                let x = (col_char - b'a') as usize;
                let y = (row_char - b'a') as usize;
                let prefix_start = (i + idx).saturating_sub(2);
                let prefix = &text[prefix_start..i + idx];
                if prefix.contains(";b") || prefix.contains(";w") {
                    moves.push((x, y));
                }
            }
        }
        i = start + 1;
    }
    moves
}

/// RenjuPortal(V1) のURLを生成する
pub fn generate_renjuportal_v1(history: &[usize]) -> String {
    let mut s = String::from("https://v1.renjuportal.com/board/?mv=");
    for &m in history {
        let x = m % 15;
        let y = m / 15;
        s.push_str(&format!("{:x}{:x}", x, y));
    }
    s
}

/// RenjuPortal(V2) のURLを生成する
pub fn generate_renjuportal_v2(history: &[usize]) -> String {
    let mut s = String::from("https://renjuportal.com/board?mvs=");
    for &m in history {
        let x = m % 15;
        let y = m / 15;
        s.push_str(&format!("{:x}{:x}", x, y));
    }
    s
}

/// RenjuPortalのURL（V1/V2）から着手座標を抽出する
pub fn parse_renjuportal(text: &str) -> Vec<(usize, usize)> {
    let mut moves = Vec::new();
    let text = text.trim();

    let mut hex_part = text;
    if let Some(query_start) = text.find('?') {
        let query = &text[query_start + 1..];
        for param in query.split('&') {
            if let Some(val) = param.strip_prefix("mv=") {
                hex_part = val;
                break;
            } else if let Some(val) = param.strip_prefix("mvs=") {
                hex_part = val;
                break;
            }
        }
    } else {
        if let Some(idx) = text.find("mv=") {
            hex_part = &text[idx + 3..];
        } else if let Some(idx) = text.find("mvs=") {
            hex_part = &text[idx + 4..];
        }
    }

    let chars: Vec<char> = hex_part.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    for chunk in chars.chunks(2) {
        if chunk.len() == 2 {
            if let (Some(x), Some(y)) = (chunk[0].to_digit(16), chunk[1].to_digit(16)) {
                if x < 15 && y < 15 {
                    moves.push((x as usize, y as usize));
                }
            }
        }
    }
    moves
}
