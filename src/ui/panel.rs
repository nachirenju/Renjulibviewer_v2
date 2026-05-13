// コメント、棋譜履歴、各種変換テキストを表示するサイドパネルの描画を行う
use eframe::egui;
use crate::lang;
use crate::core::NO_TEXT;
use crate::exportkifu::*;

pub fn draw_comment_panel_content(app: &mut crate::RenjuApp, ui: &mut egui::Ui, tr: &lang::Tr, ctx: &egui::Context) {
        ui.add_space(8.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            
            ui.heading(tr.comment);
            let current_idx = app.get_current_node();
            let mut comment_text = String::new();
            if let Some(idx) = current_idx {
                if let Some(c) = app.decode_comment(idx) { comment_text = c; }
            }
            
            let c_id = ui.make_persistent_id("comment_edit");
            let mut state = egui::widgets::text_edit::TextEditState::load(ui.ctx(), c_id).unwrap_or_default();
            let char_range = state.cursor.char_range();
            let response = ui.add_sized([ui.available_width(), 150.0], egui::TextEdit::multiline(&mut comment_text).id(c_id));
            
            if response.hovered() && ctx.input(|i| i.pointer.secondary_pressed()) {
                let mut restore_state = egui::widgets::text_edit::TextEditState::load(ui.ctx(), c_id).unwrap_or_default();
                restore_state.cursor.set_char_range(char_range);
                restore_state.store(ui.ctx(), c_id);
            }
            
            let mut changed_by_menu = false;
            response.context_menu(|ui| {
                if ui.button(tr.copy).clicked() {
                    if let Some(range) = char_range {
                        let byte_range = get_byte_range(&comment_text, range);
                        let selected = &comment_text[byte_range];
                        if !selected.is_empty() { 
                            ui.output_mut(|o| o.copied_text = selected.to_string());
                            app.trigger_copy_notification();
                        }
                    }
                    ui.close_menu();
                }
                if ui.button(tr.cut).clicked() {
                    if let Some(range) = char_range {
                        let byte_range = get_byte_range(&comment_text, range);
                        let selected = &comment_text[byte_range.clone()];
                        if !selected.is_empty() {
                            ui.output_mut(|o| o.copied_text = selected.to_string());
                            app.trigger_copy_notification();
                            comment_text.replace_range(byte_range, "");
                            let min_char = range.primary.index.min(range.secondary.index);
                            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(min_char))));
                            changed_by_menu = true;
                        }
                    }
                    ui.close_menu();
                }
                if ui.button(tr.paste).clicked() {
                    let pasted_text = ui.ctx().input(|i| {
                        i.events.iter().find_map(|e| {
                            if let egui::Event::Paste(s) = e { Some(s.clone()) } else { None }
                        })
                    });
                    if let Some(clipboard) = pasted_text {
                        let range = char_range.unwrap_or_else(|| egui::text::CCursorRange::one(egui::text::CCursor::new(0)));
                        let byte_range = get_byte_range(&comment_text, range);
                        comment_text.replace_range(byte_range, &clipboard);
                        let min_char = range.primary.index.min(range.secondary.index);
                        let new_pos = min_char + clipboard.chars().count();
                        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(new_pos))));
                        changed_by_menu = true;
                    }
                    ui.close_menu();
                }
                if changed_by_menu {
                    state.store(ui.ctx(), c_id);
                }
            });

            if response.changed() || changed_by_menu {
                let encoding = app.settings.text_encoding.to_encoding_rs();
                let new_comment_id = if comment_text.is_empty() { NO_TEXT } else { 
                    let bytes = encoding.encode(&comment_text).0.into_owned();
                    app.text_pool.as_mut().unwrap().get_or_insert(&bytes)
                };

                if let Some(nodes) = &mut app.lib_nodes {
                    if let Some(idx) = current_idx {
                        let current_hash = nodes[idx].hash;
                        if let Some(ht) = &app.hash_table {
                            if let Some(&head) = ht.get(&current_hash) {
                                app.set_comment_id(head as usize, new_comment_id);
                            }
                        }
                        app.set_comment_id(idx, new_comment_id);
                    }
                    app.subtree_cache.borrow_mut().clear();
                }
            }
            
            ui.separator();
            
            ui.horizontal(|ui| {
                ui.heading(tr.notation);
                if ui.button(tr.copy).clicked() { 
                    ui.output_mut(|o| o.copied_text = app.notation_text.clone()); 
                    app.trigger_copy_notification();
                }
            });
            let n_id = ui.make_persistent_id("notation_text");
            let mut state = egui::widgets::text_edit::TextEditState::load(ui.ctx(), n_id).unwrap_or_default();
            let char_range = state.cursor.char_range();
            if !ctx.memory(|m| m.has_focus(n_id)) { app.notation_text = generate_notation(&app.board.history); }
            let mut changed_by_menu = false;
            let n_resp = ui.add(egui::TextEdit::multiline(&mut app.notation_text).id(n_id).desired_rows(2));
            if n_resp.hovered() && ctx.input(|i| i.pointer.secondary_pressed()) {
                let mut restore_state = egui::widgets::text_edit::TextEditState::load(ui.ctx(), n_id).unwrap_or_default();
                restore_state.cursor.set_char_range(char_range);
                restore_state.store(ui.ctx(), n_id);
            }
            n_resp.context_menu(|ui| {
                if ui.button(tr.copy).clicked() {
                    if let Some(range) = char_range {
                        let byte_range = get_byte_range(&app.notation_text, range);
                        let selected = &app.notation_text[byte_range];
                        if !selected.is_empty() { 
                            ui.output_mut(|o| o.copied_text = selected.to_string());
                            app.trigger_copy_notification();
                        }
                    }
                    ui.close_menu();
                }
                if ui.button(tr.cut).clicked() {
                    if let Some(range) = char_range {
                        let byte_range = get_byte_range(&app.notation_text, range);
                        let selected = &app.notation_text[byte_range.clone()];
                        if !selected.is_empty() {
                            ui.output_mut(|o| o.copied_text = selected.to_string());
                            app.trigger_copy_notification();
                            app.notation_text.replace_range(byte_range, "");
                            let min_char = range.primary.index.min(range.secondary.index);
                            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(min_char))));
                            changed_by_menu = true;
                        }
                    }
                    ui.close_menu();
                }
                if ui.button(tr.paste).clicked() {
                    let pasted_text = ui.ctx().input(|i| {
                        i.events.iter().find_map(|e| {
                            if let egui::Event::Paste(s) = e { Some(s.clone()) } else { None }
                        })
                    });
                    if let Some(clipboard) = pasted_text {
                        let range = char_range.unwrap_or_else(|| egui::text::CCursorRange::one(egui::text::CCursor::new(0)));
                        let byte_range = get_byte_range(&app.notation_text, range);
                        app.notation_text.replace_range(byte_range, &clipboard);
                        let min_char = range.primary.index.min(range.secondary.index);
                        let new_pos = min_char + clipboard.chars().count();
                        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(new_pos))));
                        changed_by_menu = true;
                    }
                    ui.close_menu();
                }
                if changed_by_menu {
                    state.store(ui.ctx(), n_id);
                }
            });
            if n_resp.changed() || changed_by_menu {
                let moves = parse_notation(&app.notation_text);
                app.reset_to_root();
                for (x, y) in moves { app.play_move(x, y); }
            }

            ui.separator();
            
            ui.horizontal(|ui| {
                ui.heading(tr.sgf);
                if ui.button(tr.copy).clicked() { 
                    ui.output_mut(|o| o.copied_text = app.sgf_text.clone()); 
                    app.trigger_copy_notification();
                }
            });
            let s_id = ui.make_persistent_id("sgf_text");
            let mut state = egui::widgets::text_edit::TextEditState::load(ui.ctx(), s_id).unwrap_or_default();
            let char_range = state.cursor.char_range();
            if !ctx.memory(|m| m.has_focus(s_id)) { app.sgf_text = generate_sgf(&app.board.history); }
            let mut changed_by_menu = false;
            let s_resp = ui.add(egui::TextEdit::multiline(&mut app.sgf_text).id(s_id).desired_rows(3));
            if s_resp.hovered() && ctx.input(|i| i.pointer.secondary_pressed()) {
                let mut restore_state = egui::widgets::text_edit::TextEditState::load(ui.ctx(), s_id).unwrap_or_default();
                restore_state.cursor.set_char_range(char_range);
                restore_state.store(ui.ctx(), s_id);
            }
            s_resp.context_menu(|ui| {
                if ui.button(tr.copy).clicked() {
                    if let Some(range) = char_range {
                        let byte_range = get_byte_range(&app.sgf_text, range);
                        let selected = &app.sgf_text[byte_range];
                        if !selected.is_empty() { 
                            ui.output_mut(|o| o.copied_text = selected.to_string());
                            app.trigger_copy_notification();
                        }
                    }
                    ui.close_menu();
                }
                if ui.button(tr.cut).clicked() {
                    if let Some(range) = char_range {
                        let byte_range = get_byte_range(&app.sgf_text, range);
                        let selected = &app.sgf_text[byte_range.clone()];
                        if !selected.is_empty() {
                            ui.output_mut(|o| o.copied_text = selected.to_string());
                            app.trigger_copy_notification();
                            app.sgf_text.replace_range(byte_range, "");
                            let min_char = range.primary.index.min(range.secondary.index);
                            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(min_char))));
                            changed_by_menu = true;
                        }
                    }
                    ui.close_menu();
                }
                if ui.button(tr.paste).clicked() {
                    let pasted_text = ui.ctx().input(|i| {
                        i.events.iter().find_map(|e| {
                            if let egui::Event::Paste(s) = e { Some(s.clone()) } else { None }
                        })
                    });
                    if let Some(clipboard) = pasted_text {
                        let range = char_range.unwrap_or_else(|| egui::text::CCursorRange::one(egui::text::CCursor::new(0)));
                        let byte_range = get_byte_range(&app.sgf_text, range);
                        app.sgf_text.replace_range(byte_range, &clipboard);
                        let min_char = range.primary.index.min(range.secondary.index);
                        let new_pos = min_char + clipboard.chars().count();
                        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(new_pos))));
                        changed_by_menu = true;
                    }
                    ui.close_menu();
                }
                if changed_by_menu {
                    state.store(ui.ctx(), s_id);
                }
            });
            if s_resp.changed() || changed_by_menu {
                let moves = parse_sgf(&app.sgf_text);
                app.reset_to_root();
                for (x, y) in moves { app.play_move(x, y); }
            }

            ui.separator();
            
            ui.horizontal(|ui| {
                ui.heading(tr.portal_v1);
                if ui.button(tr.copy).clicked() { 
                    ui.output_mut(|o| o.copied_text = app.portal_v1_text.clone()); 
                    app.trigger_copy_notification();
                }
            });
            let v1_id = ui.make_persistent_id("portal_v1_text");
            let mut state = egui::widgets::text_edit::TextEditState::load(ui.ctx(), v1_id).unwrap_or_default();
            let char_range = state.cursor.char_range();
            if !ctx.memory(|m| m.has_focus(v1_id)) { app.portal_v1_text = generate_renjuportal_v1(&app.board.history); }
            let mut changed_by_menu = false;
            let v1_resp = ui.add(egui::TextEdit::multiline(&mut app.portal_v1_text).id(v1_id).desired_rows(2));
            if v1_resp.hovered() && ctx.input(|i| i.pointer.secondary_pressed()) {
                let mut restore_state = egui::widgets::text_edit::TextEditState::load(ui.ctx(), v1_id).unwrap_or_default();
                restore_state.cursor.set_char_range(char_range);
                restore_state.store(ui.ctx(), v1_id);
            }
            v1_resp.context_menu(|ui| {
                if ui.button(tr.copy).clicked() {
                    if let Some(range) = char_range {
                        let byte_range = get_byte_range(&app.portal_v1_text, range);
                        let selected = &app.portal_v1_text[byte_range];
                        if !selected.is_empty() { 
                            ui.output_mut(|o| o.copied_text = selected.to_string());
                            app.trigger_copy_notification();
                        }
                    }
                    ui.close_menu();
                }
                if ui.button(tr.cut).clicked() {
                    if let Some(range) = char_range {
                        let byte_range = get_byte_range(&app.portal_v1_text, range);
                        let selected = &app.portal_v1_text[byte_range.clone()];
                        if !selected.is_empty() {
                            ui.output_mut(|o| o.copied_text = selected.to_string());
                            app.trigger_copy_notification();
                            app.portal_v1_text.replace_range(byte_range, "");
                            let min_char = range.primary.index.min(range.secondary.index);
                            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(min_char))));
                            changed_by_menu = true;
                        }
                    }
                    ui.close_menu();
                }
                if ui.button(tr.paste).clicked() {
                    let pasted_text = ui.ctx().input(|i| {
                        i.events.iter().find_map(|e| {
                            if let egui::Event::Paste(s) = e { Some(s.clone()) } else { None }
                        })
                    });
                    if let Some(clipboard) = pasted_text {
                        let range = char_range.unwrap_or_else(|| egui::text::CCursorRange::one(egui::text::CCursor::new(0)));
                        let byte_range = get_byte_range(&app.portal_v1_text, range);
                        app.portal_v1_text.replace_range(byte_range, &clipboard);
                        let min_char = range.primary.index.min(range.secondary.index);
                        let new_pos = min_char + clipboard.chars().count();
                        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(new_pos))));
                        changed_by_menu = true;
                    }
                    ui.close_menu();
                }
                if changed_by_menu {
                    state.store(ui.ctx(), v1_id);
                }
            });
            if v1_resp.changed() || changed_by_menu {
                let moves = parse_renjuportal(&app.portal_v1_text);
                app.reset_to_root();
                for (x, y) in moves { app.play_move(x, y); }
            }

            ui.separator();
            
            ui.horizontal(|ui| {
                ui.heading(tr.portal_v2);
                if ui.button(tr.copy).clicked() { 
                    ui.output_mut(|o| o.copied_text = app.portal_v2_text.clone()); 
                    app.trigger_copy_notification();
                }
            });
            let v2_id = ui.make_persistent_id("portal_v2_text");
            let mut state = egui::widgets::text_edit::TextEditState::load(ui.ctx(), v2_id).unwrap_or_default();
            let char_range = state.cursor.char_range();
            if !ctx.memory(|m| m.has_focus(v2_id)) { app.portal_v2_text = generate_renjuportal_v2(&app.board.history); }
            let mut changed_by_menu = false;
            let v2_resp = ui.add(egui::TextEdit::multiline(&mut app.portal_v2_text).id(v2_id).desired_rows(2));
            if v2_resp.hovered() && ctx.input(|i| i.pointer.secondary_pressed()) {
                let mut restore_state = egui::widgets::text_edit::TextEditState::load(ui.ctx(), v2_id).unwrap_or_default();
                restore_state.cursor.set_char_range(char_range);
                restore_state.store(ui.ctx(), v2_id);
            }
            v2_resp.context_menu(|ui| {
                if ui.button(tr.copy).clicked() {
                    if let Some(range) = char_range {
                        let byte_range = get_byte_range(&app.portal_v2_text, range);
                        let selected = &app.portal_v2_text[byte_range];
                        if !selected.is_empty() { 
                            ui.output_mut(|o| o.copied_text = selected.to_string());
                            app.trigger_copy_notification();
                        }
                    }
                    ui.close_menu();
                }
                if ui.button(tr.cut).clicked() {
                    if let Some(range) = char_range {
                        let byte_range = get_byte_range(&app.portal_v2_text, range);
                        let selected = &app.portal_v2_text[byte_range.clone()];
                        if !selected.is_empty() {
                            ui.output_mut(|o| o.copied_text = selected.to_string());
                            app.trigger_copy_notification();
                            app.portal_v2_text.replace_range(byte_range, "");
                            let min_char = range.primary.index.min(range.secondary.index);
                            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(min_char))));
                            changed_by_menu = true;
                        }
                    }
                    ui.close_menu();
                }
                if ui.button(tr.paste).clicked() {
                    let pasted_text = ui.ctx().input(|i| {
                        i.events.iter().find_map(|e| {
                            if let egui::Event::Paste(s) = e { Some(s.clone()) } else { None }
                        })
                    });
                    if let Some(clipboard) = pasted_text {
                        let range = char_range.unwrap_or_else(|| egui::text::CCursorRange::one(egui::text::CCursor::new(0)));
                        let byte_range = get_byte_range(&app.portal_v2_text, range);
                        app.portal_v2_text.replace_range(byte_range, &clipboard);
                        let min_char = range.primary.index.min(range.secondary.index);
                        let new_pos = min_char + clipboard.chars().count();
                        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(new_pos))));
                        changed_by_menu = true;
                    }
                    ui.close_menu();
                }
                if changed_by_menu {
                    state.store(ui.ctx(), v2_id);
                }
            });
            if v2_resp.changed() || changed_by_menu {
                let moves = parse_renjuportal(&app.portal_v2_text);
                app.reset_to_root();
                for (x, y) in moves { app.play_move(x, y); }
            }
        });
    }

pub fn draw_comment_panel(app: &mut crate::RenjuApp, ctx: &egui::Context, tr: &lang::Tr, is_vertical: bool, screen_rect: egui::Rect) {
        if is_vertical {
            egui::TopBottomPanel::bottom("comment_panel_bottom").resizable(true).default_height(screen_rect.height() * 0.4).show(ctx, |ui| {
                crate::ui::panel::draw_comment_panel_content(app, ui, &tr, ctx);
            });
        } else {
            egui::SidePanel::right("comment_panel_right").resizable(true).default_width(280.0).show(ctx, |ui| {
                crate::ui::panel::draw_comment_panel_content(app, ui, &tr, ctx);
            });
        }
    }

/// 文字インデックスの範囲をバイトインデックスの範囲に変換する
fn get_byte_range(text: &str, char_range: egui::text::CCursorRange) -> std::ops::Range<usize> {
    let p = char_range.primary.index;
    let s = char_range.secondary.index;
    let (min_char, max_char) = if p < s { (p, s) } else { (s, p) };

    let mut char_indices = text.char_indices();
    let start_byte = char_indices.nth(min_char).map(|(i, _)| i).unwrap_or(text.len());
    let end_byte = if min_char == max_char {
        start_byte
    } else {
        char_indices.nth(max_char - min_char - 1).map(|(i, _)| i).unwrap_or(text.len())
    };
    start_byte..end_byte
}