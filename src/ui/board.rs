// 連珠盤、石、座標、およびマウス操作の描画処理を行う
use eframe::egui;
use crate::ExportState;
use crate::board::SIZE;
use crate::core::{HashBoard, NO_NODE, NO_TEXT};
use crate::board::Player;
use crate::vcf::solver::{BLACK, WHITE};

pub fn draw_main_board(app: &mut crate::RenjuApp, ctx: &egui::Context, ui: &mut egui::Ui) {
        if app.settings.show_settings_window(ctx) { crate::setup_custom_fonts(ctx, &app.settings.font_path); }

        crate::ui::dialogs::draw_about_dialog(app, ctx);
        crate::ui::dialogs::draw_gif_setup_dialog(app, ctx);
        ui.add_space(10.0);
        
        let available = ui.available_size();
        let board_size = (available.x.min(available.y) * 0.95).max(200.0);
        let (response, painter) = ui.allocate_painter(egui::Vec2::splat(board_size), egui::Sense::click_and_drag());
        let rect = response.rect;
        let cell_size = rect.width() / (SIZE as f32 + 1.0);

        let is_gif_capturing = matches!(app.export_state, ExportState::GifWaitCapture(_, _) | ExportState::GifCaptureRequested(_, _));
        let is_selecting_region = matches!(app.export_state, ExportState::SelectingRegion(_, _) | ExportState::GifSelectingRegion(_, _));

        let panel_right_clicked = ui.max_rect().contains(ctx.input(|i| i.pointer.interact_pos()).unwrap_or_default()) 
            && ctx.input(|i| i.pointer.secondary_clicked());
        crate::ui::board::handle_board_input(app, ctx, &response, rect, cell_size, is_gif_capturing, is_selecting_region, panel_right_clicked);

        crate::ui::board::draw_grid_and_coords(app, &painter, rect, cell_size);

            // --- ★計算: 現在の局面状態を取得 (不変借用) ---
            let gif_current_move = match app.export_state {
                ExportState::GifWaitCapture(_, c) => Some(c),
                ExportState::GifCaptureRequested(_, c) => Some(c),
                _ => None,
            };

            let current_history = if let Some(c) = gif_current_move {
                &app.board.history[0..c.min(app.board.history.len())]
            } else {
                &app.board.history[..]
            };

            let target_idx = if is_gif_capturing {
                if app.is_db_mode {
                    // --- DBモードの処理はそのまま ---
                    let mut pb = HashBoard::new();
                    for &m in current_history {
                        pb.make_move((m % 15) as i8, (m / 15) as i8);
                    }
                    let h = pb.get_canonical_hash();
                    let mut found_idx = None;
                    if let Some(ht) = &app.hash_table {
                        if let Some(&head) = ht.get(&h) {
                            found_idx = Some(head as usize);
                            if let Some(&i) = ht.get(&h) {
                                let i = i as usize;
                                if app.get_comment_id(i) != NO_TEXT || app.get_text_id(i) != NO_TEXT {
                                    found_idx = Some(i);
                                }
                            }
                        }
                    }
                    found_idx
                } else {
                    // LIBモード：終点から親(parent)を遡って正確なノードを特定する 
                    let mut target = app.get_current_node();
                    // 現在の全体の履歴数から、GIFの現在のコマの手数を引いて、何手戻ればいいかを計算
                    let moves_to_undo = app.board.history.len().saturating_sub(current_history.len());
                    
                    if let Some(nodes) = &app.lib_nodes {
                        for _ in 0..moves_to_undo {
                            if let Some(idx) = target {
                                let p = nodes[idx].parent;
                                if p != NO_NODE {
                                    target = Some(p as usize);
                                } else {
                                    break;
                                }
                            }
                        }
                        target
                    } else {
                        None
                    }
                    // ------------------------------------------------------------------------
                }
           
            } else {
                app.get_current_node()
            };
            // --------------------------------------------


        crate::ui::board::draw_stones(app, &painter, rect, cell_size, current_history, is_gif_capturing);

        if app.settings.show_branches && !is_gif_capturing {
            crate::ui::board::draw_branches(app, ctx, &painter, rect, cell_size, current_history, target_idx);
        }

        if !is_gif_capturing {
            if app.settings.show_forbidden_points {
                crate::ui::board::draw_forbidden_points(app, &painter, rect, cell_size);
            }
            crate::ui::board::draw_vcf_solution(app, &painter, rect, cell_size);
        }

            // ==========================================
            // キャプチャの処理
            // ==========================================

            if app.export_state == ExportState::RequestFull {
                app.export_state = ExportState::WaitCapture(rect);
            } else if app.export_state == ExportState::GifRequestFull {
                let start_m = app.gif_settings.start_move.clamp(0, app.board.history.len());
                app.export_state = ExportState::GifWaitCapture(rect, start_m);
            }

            let mut next_state = app.export_state.clone();
            if let ExportState::SelectingRegion(ref mut start_pos, ref mut curr_rect) = next_state {
                let pointer = ctx.input(|i| i.pointer.clone());
                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) { next_state = ExportState::Idle; }
                else {
                    if pointer.primary_pressed() { if let Some(pos) = pointer.interact_pos() { *start_pos = Some(pos); } }
                    if let Some(start) = start_pos {
                        if let Some(pos) = pointer.interact_pos() {
                            let r = egui::Rect::from_two_pos(*start, pos);
                            *curr_rect = Some(r);
                            let overlay_painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("export_region_layer")));
                            overlay_painter.rect_stroke(r, 0.0, (2.0, egui::Color32::RED));
                        }
                        if pointer.primary_released() {
                            if let Some(r) = *curr_rect {
                                next_state = ExportState::WaitCapture(r);
                            } else { next_state = ExportState::Idle; }
                        }
                    } else { ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Crosshair); }
                }
            }
            if let ExportState::GifSelectingRegion(ref mut start_pos, ref mut curr_rect) = next_state {
                let pointer = ctx.input(|i| i.pointer.clone());
                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) { next_state = ExportState::Idle; }
                else {
                    if pointer.primary_pressed() { if let Some(pos) = pointer.interact_pos() { *start_pos = Some(pos); } }
                    if let Some(start) = start_pos {
                        if let Some(pos) = pointer.interact_pos() {
                            let r = egui::Rect::from_two_pos(*start, pos);
                            *curr_rect = Some(r);
                            let overlay_painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("export_region_layer")));
                            overlay_painter.rect_stroke(r, 0.0, (2.0, egui::Color32::RED));
                        }
                        if pointer.primary_released() {
                            if let Some(r) = *curr_rect {
                                let start_m = app.gif_settings.start_move.clamp(0, app.board.history.len());
                                next_state = ExportState::GifWaitCapture(r, start_m);
                            } else { next_state = ExportState::Idle; }
                        }
                    } else { ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Crosshair); }
                }
            }
            app.export_state = next_state;

            // GIFキャプチャ中はeguiをスリープさせず、常にフルスピードでイベントループを回す
            if matches!(app.export_state, ExportState::GifWaitCapture(_, _) | ExportState::GifCaptureRequested(_, _)) {
            ctx.request_repaint();
            }

            let mut issue_capture = false;
            let mut target_rect = egui::Rect::NOTHING;
            let mut is_gif = false;
            let mut current_move = 0;

            if let ExportState::WaitCapture(target) = app.export_state {
                issue_capture = true; target_rect = target; 
            } else if let ExportState::GifWaitCapture(target, cm) = app.export_state {
                issue_capture = true; target_rect = target; current_move = cm; is_gif = true; 
            }

            if issue_capture {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                if is_gif {
                    app.export_state = ExportState::GifCaptureRequested(target_rect, current_move);
                } else {
                    app.export_state = ExportState::CaptureRequested(target_rect);
                }
            }

            if let Some(filename) = &app.current_file {
                ui.vertical_centered(|ui| {
                    ui.add_space(10.0);
                    let node_count = app.lib_nodes.as_ref().map_or(0, |nodes| nodes.len());
                    let mut display_text = format!("{} (Total Nodes: {})", filename, node_count);
                    if let Some(time) = app.load_time_sec { display_text.push_str(&format!(" [Load time: {:.2}s]", time)); }
                    ui.label(egui::RichText::new(display_text).strong().color(egui::Color32::DARK_GRAY));
                });
            }
    }

pub fn draw_forbidden_points(app: &mut crate::RenjuApp, painter: &egui::Painter, rect: egui::Rect, cell_size: f32) {
            let points = app.cached_forbidden_points();
            let stroke = egui::Stroke::new((cell_size * 0.08).max(2.0), egui::Color32::RED);
            let radius = cell_size * 0.24;
            for &idx in points {
                let x = idx % SIZE;
                let y = idx / SIZE;
                let center = egui::pos2(rect.left() + cell_size * (x as f32 + 1.0), rect.top() + cell_size * (y as f32 + 1.0));
                painter.line_segment([center + egui::vec2(-radius, -radius), center + egui::vec2(radius, radius)], stroke);
                painter.line_segment([center + egui::vec2(-radius, radius), center + egui::vec2(radius, -radius)], stroke);
            }
    }

pub fn draw_vcf_solution(app: &crate::RenjuApp, painter: &egui::Painter, rect: egui::Rect, cell_size: f32) {
            if app.vcf_solution.is_empty() {
                return;
            }
            let font_id = egui::FontId::proportional(cell_size * 0.28);
            for (i, mv) in app.vcf_solution.iter().take(app.vcf_replay_len).enumerate() {
                let center = egui::pos2(rect.left() + cell_size * (mv.x as f32 + 1.0), rect.top() + cell_size * (mv.y as f32 + 1.0));
                let fill = if mv.color == BLACK {
                    egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180)
                } else if mv.color == WHITE {
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 210)
                } else {
                    egui::Color32::from_rgba_unmultiplied(160, 160, 160, 180)
                };
                let text_color = if mv.color == BLACK { egui::Color32::WHITE } else { egui::Color32::BLACK };
                let radius = cell_size * 0.4;
                painter.circle_filled(center, radius, fill);
                painter.circle_stroke(center, radius, (1.0, egui::Color32::BLACK));
                painter.text(center, egui::Align2::CENTER_CENTER, (i + 1).to_string(), font_id.clone(), text_color);
            }
    }

pub fn handle_board_input(app: &mut crate::RenjuApp, ctx: &egui::Context, response: &egui::Response, rect: egui::Rect, cell_size: f32, is_gif_capturing: bool, is_selecting_region: bool, panel_right_clicked: bool) {
            if !is_selecting_region && !is_gif_capturing && (response.clicked() || (response.drag_stopped() && response.dragged_by(egui::PointerButton::Primary))) {
                if let Some(pos) = response.interact_pointer_pos() {
                    let gx = ((pos.x - (rect.left() + cell_size)) / cell_size + 0.5).floor() as i32;
                    let gy = ((pos.y - (rect.top() + cell_size)) / cell_size + 0.5).floor() as i32;
                    if gx >= 0 && gx < 15 && gy >= 0 && gy < 15 {
                        
                        if ctx.input(|i| i.modifiers.ctrl) || app.is_text_mode {
                            app.editing_coord = Some((gx as i8, gy as i8));
                            app.editing_text = String::new();

                            let new_hash = {
                                let mut pb = HashBoard::new();
                                for &m in &app.board.history { pb.make_move((m % 15) as i8, (m / 15) as i8); }
                                pb.make_move(gx as i8, gy as i8);
                                pb.get_canonical_hash()
                            };

                            if let Some(ht) = &app.hash_table {
                                if let Some(&head) = ht.get(&new_hash) {
                                    if let Some(t) = app.decode_text(head as usize) {
                                        app.editing_text = t;
                                    }
                                }
                            }
                        } else {
                            app.play_move(gx as usize, gy as usize);
                        }
                    }
                }
            }

            let right_clicked = response.secondary_clicked() || panel_right_clicked;
            if right_clicked && !is_gif_capturing {
                let before_len = app.board.history.len();
                if app.is_db_mode { app.board.undo(); } else if let Some(idx) = app.current_node_idx {
                    if let Some(nodes) = &app.lib_nodes {
                        let p_idx = nodes[idx].parent;
                        if p_idx != NO_NODE {
                            if nodes[idx].x() >= 0 { app.board.undo(); }
                            app.current_node_idx = Some(p_idx as usize);
                        }
                    }
                }
                if app.board.history.len() != before_len {
                    app.clear_vcf_solution();
                }
            }

    }

pub fn draw_grid_and_coords(app: &crate::RenjuApp, painter: &egui::Painter, rect: egui::Rect, cell_size: f32) {
            painter.rect_filled(rect, 0.0, app.settings.board_color32());
            for i in 0..SIZE {
                let pos = cell_size * (i as f32 + 1.0);
                painter.line_segment([egui::pos2(rect.left() + cell_size, rect.top() + pos), egui::pos2(rect.right() - cell_size, rect.top() + pos)], (1.0, egui::Color32::BLACK));
                painter.line_segment([egui::pos2(rect.left() + pos, rect.top() + cell_size), egui::pos2(rect.left() + pos, rect.bottom() - cell_size)], (1.0, egui::Color32::BLACK));
            }
            
            for (sx, sy) in [(7, 7), (11, 3), (11, 11), (3, 3), (3, 11)] {
                let center = egui::pos2(rect.left() + cell_size * (sx as f32 + 1.0), rect.top() + cell_size * (sy as f32 + 1.0));
                painter.circle_filled(center, cell_size * 0.08, egui::Color32::BLACK);
            }

            let font_id = egui::FontId::proportional(cell_size * 0.4);
            for i in 0..SIZE {
                let pos = cell_size * (i as f32 + 1.0);
                painter.text(egui::pos2(rect.left() + cell_size * 0.4, rect.top() + pos), egui::Align2::CENTER_CENTER, (SIZE - i).to_string(), font_id.clone(), egui::Color32::BLACK);
                painter.text(egui::pos2(rect.left() + pos, rect.bottom() - cell_size * 0.4), egui::Align2::CENTER_CENTER, ((b'A' + i as u8) as char).to_string(), font_id.clone(), egui::Color32::BLACK);
            }

        let font_id = egui::FontId::proportional(cell_size * 0.4);
            for (&(lx, ly), text) in &app.ad_hoc_labels {
                let center = egui::pos2(rect.left() + cell_size * (lx as f32 + 1.0), rect.top() + cell_size * (ly as f32 + 1.0));
                let bg_color = egui::Color32::from_rgb(249, 235, 207);
                painter.rect_filled(egui::Rect::from_center_size(center, egui::vec2(cell_size * 0.8, cell_size * 0.4)), 2.0, bg_color);
                painter.text(center, egui::Align2::CENTER_CENTER, text, font_id.clone(), egui::Color32::BLACK);
            }

    }

pub fn draw_stones(app: &crate::RenjuApp, painter: &egui::Painter, rect: egui::Rect, cell_size: f32, current_history: &[usize], is_gif_capturing: bool) {
let font_id = egui::FontId::proportional(cell_size * 0.4);
            let mut temp_grid = [None; SIZE * SIZE];
            let mut temp_player = Player::Black;
            for &m in current_history {
                temp_grid[m] = Some(temp_player);
                temp_player = match temp_player {
                    Player::Black => Player::White,
                    Player::White => Player::Black,
                };
            }

            for y in 0..SIZE {
                for x in 0..SIZE {
                    if let Some(player) = temp_grid[y * SIZE + x] {
                        let center = egui::pos2(rect.left() + cell_size * (x as f32 + 1.0), rect.top() + cell_size * (y as f32 + 1.0));
                        
                        if app.settings.stone_shading {
                            let base_radius = cell_size * 0.4;
                            if player == Player::Black {
                                for i in 0..=30 {
                                    let t = i as f32 / 30.0;
                                    let ease = t.sqrt(); 
                                    let radius = base_radius * (1.0 - t);
                                    let offset = base_radius * 0.25 * t;
                                    let current_center = center + egui::vec2(-offset, -offset);
                                    let color_val = (102.0 * ease) as u8;
                                    painter.circle_filled(current_center, radius, egui::Color32::from_rgb(color_val, color_val, color_val));
                                }
                                painter.circle_stroke(center, base_radius, (1.0, egui::Color32::from_gray(30)));
                            } else {
                                for i in 0..=30 {
                                    let t = i as f32 / 30.0;
                                    let ease = t.sqrt();
                                    let radius = base_radius * (1.0 - t);
                                    let offset = base_radius * 0.25 * t;
                                    let current_center = center + egui::vec2(-offset, -offset);
                                    let color_val = (204.0 + (255.0 - 204.0) * ease) as u8;
                                    painter.circle_filled(current_center, radius, egui::Color32::from_rgb(color_val, color_val, color_val));
                                }
                                painter.circle_stroke(center, base_radius, (1.0, egui::Color32::from_gray(180)));
                            }
                        } else {
                            let color = if player == Player::Black { egui::Color32::BLACK } else { egui::Color32::WHITE };
                            painter.circle_filled(center, cell_size * 0.4, color);
                            if player == Player::White { painter.circle_stroke(center, cell_size * 0.4, (1.0, egui::Color32::BLACK)); }
                        }

                        if app.settings.show_numbers {
                            if let Some(turn) = current_history.iter().position(|&idx| idx == y * SIZE + x) {
                                let move_number = turn + 1;
                                let show = if is_gif_capturing { move_number >= app.gif_settings.show_number_start_move } else { true };
                                
                                if show {
                                    let display_num = if is_gif_capturing {
                                        app.gif_settings.show_number_start_value + (move_number - app.gif_settings.show_number_start_move)
                                    } else {
                                        move_number
                                    };
                                    let t_color = if move_number == current_history.len() {
                                        egui::Color32::from_rgb(
                                            (app.settings.last_move_color[0] * 255.0) as u8,
                                            (app.settings.last_move_color[1] * 255.0) as u8,
                                            (app.settings.last_move_color[2] * 255.0) as u8,
                                        )
                                    } else if player == Player::Black {
                                        egui::Color32::WHITE
                                    } else {
                                        egui::Color32::BLACK
                                    };
                                    painter.text(center, egui::Align2::CENTER_CENTER, display_num.to_string(), font_id.clone(), t_color);
                                }
                            }
                        }
                    }
                }
            }

    }

pub fn draw_branches(app: &crate::RenjuApp, ctx: &egui::Context, painter: &egui::Painter, rect: egui::Rect, cell_size: f32, current_history: &[usize], target_idx: Option<usize>) {
                if let Some(nodes) = &app.lib_nodes {
                let mut pb = HashBoard::new();
                for &m in current_history { pb.make_move((m % 15) as i8, (m / 15) as i8); }

                #[derive(PartialEq)]
                enum MoveType { Normal, Symmetry, Merge }

                let is_black_turn = current_history.len() % 2 == 0;

                let mut direct_children = Vec::new();
                let mut child_hashes = std::collections::HashSet::new();
                if !app.is_db_mode {
                    if let Some(idx) = target_idx {
                        let mut c_idx = nodes[idx].child;
                        while c_idx != NO_NODE {
                            let node = &nodes[c_idx as usize];
                            if node.x() >= 0 {
                                direct_children.push((node.x() as i32, node.y() as i32, c_idx as usize));
                                let mut sim_pb = pb.clone();
                                sim_pb.make_move(node.x(), node.y());
                                child_hashes.insert(sim_pb.get_canonical_hash());
                            }
                            c_idx = node.sibling;
                        }
                    }
                }

                struct BranchInfo {
                    x: usize,
                    y: usize,
                    target_node_idx: Option<usize>,
                    count: usize,
                    text: Option<String>,
                }
                
                let mut branches_to_draw = Vec::new();

                for y in 0..15 {
                    for x in 0..15 {
                        if app.board.grid[y * 15 + x].is_none() {
                            pb.make_move(x as i8, y as i8);
                            let h = pb.get_canonical_hash();
                            pb.undo_move(x as i8, y as i8);

                            let mut move_type = None;
                            let mut target_node_idx = None;

                            let is_direct = direct_children.iter().find(|&&(cx, cy, _)| cx == x as i32 && cy == y as i32);
                            let direct_c_idx = is_direct.map(|&(_, _, c_idx)| c_idx);

                            if let Some(&head) = app.hash_table.as_ref().and_then(|ht| ht.get(&h)) {
                                let mut has_other_branches = false;
                                let mut explicit_node = None;
                                let mut node_with_text = None;
                                let first_node = head as usize;

                                let i = head as usize;
                                if nodes[i].x() == x as i8 && nodes[i].y() == y as i8 { explicit_node = Some(i); }
                                if Some(i) != direct_c_idx { has_other_branches = true; }
                                if app.get_text_id(i) != NO_TEXT { node_with_text = Some(i); }
                                
                                if has_other_branches {
                                    if explicit_node.is_some() { move_type = Some(MoveType::Merge); } else { move_type = Some(MoveType::Symmetry); }
                                } else if direct_c_idx.is_some() { move_type = Some(MoveType::Normal); }

                                target_node_idx = node_with_text.or(explicit_node).or(direct_c_idx).or(Some(first_node));
                                    
                            } else if let Some(c_idx) = direct_c_idx {
                                move_type = Some(MoveType::Normal);
                                target_node_idx = Some(c_idx);
                            } else if child_hashes.contains(&h) {
                                move_type = Some(MoveType::Symmetry);
                                for &(cx, cy, c_idx) in &direct_children {
                                    let mut sim_pb = pb.clone();
                                    sim_pb.make_move(cx as i8, cy as i8);
                                    if sim_pb.get_canonical_hash() == h { target_node_idx = Some(c_idx); break; }
                                }
                            }

                            if move_type.is_some() {
                                let mut text = None;

                                if let Some(idx) = target_node_idx {
                                    text = app.decode_text(idx);
                                }

                                let mut max_c = 0;

                                if let Some(&head) = app.hash_table.as_ref().and_then(|ht| ht.get(&h)) {
                                    max_c = app.get_subtree_size(head as usize);
                                }

                                // (B) 手順前後で1手先に巨大な合流ツリーが隠れているケースを拾うための「1手先読み」
                                if app.is_db_mode {
                                    let mut lookahead_max = 0;
                                    let mut temp_pb = pb.clone();
                                    // 候補手 (x, y) を進めて仮想的な次の盤面を作る
                                    temp_pb.make_move(x as i8, y as i8);
                                    
                                    for ny in 0..15 {
                                        for nx in 0..15 {
                                            let idx = ny * 15 + nx;
                                            // 空きマスかどうかの判定 (temp_pb.occupied を使う)
                                            if (temp_pb.occupied[idx / 64] & (1 << (idx % 64))) == 0 {
                                                temp_pb.make_move(nx as i8, ny as i8);
                                                let h_next = temp_pb.get_canonical_hash();
                                                temp_pb.undo_move(nx as i8, ny as i8);

                                                // 1手先のハッシュがDBに存在すれば、そのツリーサイズを確認
                                                if let Some(&next_head) = app.hash_table.as_ref().and_then(|ht| ht.get(&h_next)) {
                                                    let c = app.get_subtree_size(next_head as usize);
                                                    if c > lookahead_max { lookahead_max = c; }
                                                }
                                            }
                                        }
                                    }
                                    // 直接のサイズよりも1手先の最大サイズの方が大きければ、それを採用する
                                    if lookahead_max > max_c {
                                        max_c = lookahead_max;
                                    }
                                }

                                let count = max_c;

                                branches_to_draw.push(BranchInfo {
                                    x, y, target_node_idx, count, text
                                });
                            }
                        }
                    }
                }

                let mut counts_with_idx: Vec<(usize, usize)> = branches_to_draw.iter().enumerate()
                    .map(|(i, b)| (i, b.count)).collect();
                counts_with_idx.sort_unstable_by(|a, b| b.1.cmp(&a.1)); 

                let mut highlight_indices = std::collections::HashSet::new();
                for (rank, &(idx, count)) in counts_with_idx.iter().enumerate() {
                    if rank < app.settings.top_branch_highlight_count && count > 0 { 
                        highlight_indices.insert(idx);
                    }
                }

                let alpha = 140; 
                let bg_color = if is_black_turn { 
                    let c = egui::Color32::from_gray(160);
                    egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha) 
                } else { 
                    let c = egui::Color32::WHITE;
                    egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha) 
                };
                let s_c = egui::Color32::from_gray(80);
                let stroke_color = egui::Color32::from_rgba_unmultiplied(s_c.r(), s_c.g(), s_c.b(), alpha);

                for (i, branch) in branches_to_draw.iter().enumerate() {
                    let center = egui::pos2(rect.left() + cell_size * (branch.x as f32 + 1.0), rect.top() + cell_size * (branch.y as f32 + 1.0));
                    
                    painter.circle_filled(center, cell_size * 0.4, bg_color);
                    painter.circle_stroke(center, cell_size * 0.4, (1.0, stroke_color));

                    let is_highlighted = highlight_indices.contains(&i);

                    let mut child_count_text = String::new();
                    if app.settings.show_branch_count && branch.target_node_idx.is_some() {
                        let count = branch.count;
                        child_count_text = if count >= 1_000_000 {
                            let m = (count as f64 / 100_000.0).round() / 10.0;
                            if m.fract() == 0.0 { format!("{}m", m as usize) } else { format!("{:.1}m", m) }
                        } else if count >= 1000 {
                            let k = (count as f64 / 100.0).round() / 10.0;
                            if k.fract() == 0.0 { format!("{}k", k as usize) } else { format!("{:.1}k", k) }
                        } else if count > 0 {
                            count.to_string()
                        } else {
                            String::new()
                        };
                    }

                    let highlight_font_id = egui::FontId::proportional(cell_size * 0.26);
                    let normal_font_id = egui::FontId::proportional(cell_size * 0.22);
                    let current_font = if is_highlighted { highlight_font_id } else { normal_font_id };
                    
                    let text_color = if is_highlighted {
                        egui::Color32::RED
                    } else {
                        egui::Color32::BLACK
                    };

                    let draw_text_tight = |painter: &egui::Painter, pos: egui::Pos2, text: &str| {
                        if text.contains('.') {
                            let parts: Vec<&str> = text.split('.').collect();
                            if parts.len() == 2 {
                                let t1 = parts[0];
                                let t2 = parts[1];
                                
                                let g1 = ctx.fonts(|f| f.layout_no_wrap(t1.to_string(), current_font.clone(), text_color));
                                let g_dot = ctx.fonts(|f| f.layout_no_wrap(".".to_string(), current_font.clone(), text_color));
                                let g2 = ctx.fonts(|f| f.layout_no_wrap(t2.to_string(), current_font.clone(), text_color));
                                
                                let dot_width_used = g_dot.rect.width() * 0.3; 
                                let gap = cell_size * 0.01;
                                
                                let total_width = g1.rect.width() + gap + dot_width_used + gap + g2.rect.width();
                                let mut start_x = pos.x - total_width / 2.0;
                                
                                let draw_galley = |p: &egui::Painter, x: f32, g: std::sync::Arc<egui::Galley>| {
                                    let draw_pos = egui::pos2(x, pos.y - g.rect.height() / 2.0);
                                    p.galley(draw_pos, g.clone(), text_color);
                                    if is_highlighted {
                                        p.galley(draw_pos + egui::vec2(0.5, 0.0), g.clone(), text_color);
                                        p.galley(draw_pos + egui::vec2(0.0, 0.5), g, text_color);
                                    }
                                };
                                
                                draw_galley(painter, start_x, g1.clone());
                                start_x += g1.rect.width() + gap;
                                draw_galley(painter, start_x, g_dot.clone());
                                start_x += dot_width_used + gap;
                                draw_galley(painter, start_x, g2.clone());
                                
                                return;
                            }
                        }
                        
                        let g = ctx.fonts(|f| f.layout_no_wrap(text.to_string(), current_font.clone(), text_color));
                        let draw_pos = egui::pos2(pos.x - g.rect.width() / 2.0, pos.y - g.rect.height() / 2.0);
                        painter.galley(draw_pos, g.clone(), text_color);
                        if is_highlighted {
                            painter.galley(draw_pos + egui::vec2(0.5, 0.0), g.clone(), text_color);
                            painter.galley(draw_pos + egui::vec2(0.0, 0.5), g, text_color);
                        }
                    };

                    let draw_custom_label = |painter: &egui::Painter, pos: egui::Pos2, text: &str| {
                        let label_font = egui::FontId::proportional(cell_size * 0.32);
                        let g = ctx.fonts(|f| f.layout_no_wrap(text.to_string(), label_font, egui::Color32::BLACK));
                        let draw_pos = egui::pos2(pos.x - g.rect.width() / 2.0, pos.y - g.rect.height() / 2.0);
                        painter.galley(draw_pos, g, egui::Color32::BLACK);
                    };

                    let has_text = app.settings.show_branch_text && branch.text.is_some();
                    let text_to_draw = if has_text { branch.text.as_ref() } else { None };

                    if let Some(t) = text_to_draw {
                        if !child_count_text.is_empty() {
                            let text_pos = center - egui::vec2(0.0, cell_size * 0.14);
                            let count_pos = center + egui::vec2(0.0, cell_size * 0.16);
                            draw_custom_label(&painter, text_pos, t);
                            draw_text_tight(&painter, count_pos, &child_count_text);
                        } else {
                            draw_custom_label(&painter, center, t);
                        }
                    } else if !child_count_text.is_empty() {
                        draw_text_tight(&painter, center, &child_count_text);
                    }
                }
            }
    }
