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
            
            let response = ui.add_sized([ui.available_width(), 150.0], egui::TextEdit::multiline(&mut comment_text));
            
            if response.changed() {
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
                if ui.button(tr.copy).clicked() { ui.output_mut(|o| o.copied_text = app.notation_text.clone()); }
            });
            let n_id = ui.make_persistent_id("notation_text");
            if !ctx.memory(|m| m.has_focus(n_id)) { app.notation_text = generate_notation(&app.board.history); }
            if ui.add(egui::TextEdit::multiline(&mut app.notation_text).id(n_id).desired_rows(2)).changed() {
                let moves = parse_notation(&app.notation_text);
                app.reset_to_root();
                for (x, y) in moves { app.play_move(x, y); }
            }

            ui.separator();
            
            ui.horizontal(|ui| {
                ui.heading(tr.sgf);
                if ui.button(tr.copy).clicked() { ui.output_mut(|o| o.copied_text = app.sgf_text.clone()); }
            });
            let s_id = ui.make_persistent_id("sgf_text");
            if !ctx.memory(|m| m.has_focus(s_id)) { app.sgf_text = generate_sgf(&app.board.history); }
            if ui.add(egui::TextEdit::multiline(&mut app.sgf_text).id(s_id).desired_rows(3)).changed() {
                let moves = parse_sgf(&app.sgf_text);
                app.reset_to_root();
                for (x, y) in moves { app.play_move(x, y); }
            }

            ui.separator();
            
            ui.horizontal(|ui| {
                ui.heading(tr.portal_v1);
                if ui.button(tr.copy).clicked() { ui.output_mut(|o| o.copied_text = app.portal_v1_text.clone()); }
            });
            let v1_id = ui.make_persistent_id("portal_v1_text");
            if !ctx.memory(|m| m.has_focus(v1_id)) { app.portal_v1_text = generate_renjuportal_v1(&app.board.history); }
            if ui.add(egui::TextEdit::multiline(&mut app.portal_v1_text).id(v1_id).desired_rows(2)).changed() {
                let moves = parse_renjuportal(&app.portal_v1_text);
                app.reset_to_root();
                for (x, y) in moves { app.play_move(x, y); }
            }

            ui.separator();
            
            ui.horizontal(|ui| {
                ui.heading(tr.portal_v2);
                if ui.button(tr.copy).clicked() { ui.output_mut(|o| o.copied_text = app.portal_v2_text.clone()); }
            });
            let v2_id = ui.make_persistent_id("portal_v2_text");
            if !ctx.memory(|m| m.has_focus(v2_id)) { app.portal_v2_text = generate_renjuportal_v2(&app.board.history); }
            if ui.add(egui::TextEdit::multiline(&mut app.portal_v2_text).id(v2_id).desired_rows(2)).changed() {
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