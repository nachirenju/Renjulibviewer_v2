// 画面上部のツールバー（ファイル操作、編集、保存、リセット）の描画を行う
use eframe::egui;
use crate::lang;
use crate::ExportState;
use crate::core::NO_NODE;
pub fn draw_toolbar(app: &mut crate::RenjuApp, ctx: &egui::Context, tr: &lang::Tr) {
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let (btn_size, spacing) = if cfg!(target_arch = "wasm32") {
                    (app.settings.wasm_button_size, app.settings.wasm_button_size * 0.5)
                } else { (20.0, 12.0) };

                ui.spacing_mut().item_spacing.x = spacing;

                if ui.button(egui::RichText::new(tr.new_game).size(btn_size)).on_hover_text(tr.new_game_tooltip).clicked() {
                    app.show_new_game_confirm = true;
                }

                if ui.button(egui::RichText::new(tr.settings).size(btn_size)).on_hover_text(tr.settings_tooltip).clicked() {
                    app.settings.show_window = !app.settings.show_window;
                }
                
                if ui.button(egui::RichText::new(tr.open_file).size(btn_size)).on_hover_text(tr.open_file_tooltip).clicked() {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        if let Some(path) = rfd::FileDialog::new().add_filter("Renju Files", &["lib", "db"]).pick_file() {
                            app.load_file(&path);
                        }
                    }

                    #[cfg(target_arch = "wasm32")]
                    {
                        let dropped_file_clone = app.wasm_dropped_file.clone();
                        let ctx_clone = ctx.clone();
                        wasm_bindgen_futures::spawn_local(async move {
                            if let Some(file) = rfd::AsyncFileDialog::new().add_filter("Renju", &["lib", "db"]).pick_file().await {
                                let file_name = file.file_name();
                                let data = file.read().await; 
                                if let Ok(mut lock) = dropped_file_clone.lock() { *lock = Some((file_name, data)); }
                                ctx_clone.request_repaint();
                            }
                        });
                    }
                }
                
                ui.menu_button(egui::RichText::new(tr.save_as).size(btn_size), |ui| {
                    if ui.button(tr.save_overwrite).clicked() {
                        app.overwrite_save();
                        ui.close_menu();
                    }
                    if ui.button(tr.save_as_menu).clicked() {
                        app.save_file_dialog(false);
                        ui.close_menu();
                    }
                    if ui.button(tr.save_branch).clicked() {
                        app.save_file_dialog(true);
                        ui.close_menu();
                    }
                }).response.on_hover_text(tr.save_as_tooltip);
                
                ui.separator();
                
                if ui.button(egui::RichText::new(tr.undo_all).size(btn_size)).on_hover_text(tr.undo_all_tooltip).clicked() { app.reset_to_root(); }
                
                if ui.button(egui::RichText::new(tr.undo_one).size(btn_size)).on_hover_text(tr.undo_one_tooltip).clicked() {
                    let before_len = app.board.history.len();
                    if app.is_db_mode { app.board.undo(); } else if let Some(nodes) = &app.lib_nodes {
                        if let Some(idx) = app.current_node_idx {
                            let node = &nodes[idx];
                            let p_idx = node.parent;
                            if p_idx != NO_NODE {
                                if node.x() >= 0 && node.y() >= 0 { app.board.undo(); }
                                app.current_node_idx = Some(p_idx as usize);
                            }
                        }
                    } else { app.board.undo(); }
                    if app.board.history.len() != before_len {
                        app.clear_vcf_solution();
                    }
                }
                
                if ui.button(egui::RichText::new(tr.redo_one).size(btn_size)).on_hover_text(tr.redo_one_tooltip).clicked() {
                    app.step_forward();
                }
                
                if ui.button(egui::RichText::new(tr.redo_all).size(btn_size)).on_hover_text(tr.redo_all_tooltip).clicked() {
                    while app.step_forward() {}
                }

                ui.separator(); 
                
                if ui.button(egui::RichText::new("🗑").size(btn_size)).on_hover_text(tr.delete_confirm_title).clicked() {
                    if app.node_to_delete.is_none() && app.editing_coord.is_none() {
                        if let Some(idx) = app.get_current_node() {
                            if idx > 0 { app.node_to_delete = Some(idx); }
                        }
                    }
                }

                if ui.toggle_value(&mut app.is_text_mode, egui::RichText::new("📝").size(btn_size)).on_hover_text("Text Edit Mode").clicked() {
                    // is_text_mode is toggled automatically
                }

                ui.menu_button(egui::RichText::new("📷").size(btn_size), |ui| {
                    if ui.button("盤面全体を保存").clicked() {
                        app.export_state = ExportState::RequestFull;
                        ui.close_menu();
                    }
                    if ui.button("範囲を指定して保存").clicked() {
                        app.export_state = ExportState::SelectingRegion(None, None);
                        ui.close_menu();
                    }
                }).response.on_hover_text("画像として保存");

                if ui.button(egui::RichText::new("🎬").size(btn_size)).on_hover_text("GIFアニメとして保存").clicked() {
                    app.show_gif_setup = true;
                    app.gif_settings.end_move = app.board.history.len();
                }

                if ui.button(egui::RichText::new("ℹ️").size(btn_size)).on_hover_text("About").clicked() {
                    app.show_about = true;
                }
            });
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let (btn_size, spacing) = if cfg!(target_arch = "wasm32") {
                    (app.settings.wasm_button_size * 0.8, app.settings.wasm_button_size * 0.35)
                } else { (16.0, 8.0) };

                ui.spacing_mut().item_spacing.x = spacing;
                if ui.add_enabled(!app.vcf_solving, egui::Button::new(egui::RichText::new(tr.vcf).size(btn_size).strong())).clicked() {
                    app.start_vcf_search();
                }

                if app.vcf_solving {
                    ui.spinner();
                    ui.label(egui::RichText::new(tr.vcf_solving).size(btn_size));
                } else if !app.vcf_status.is_empty() {
                    ui.label(egui::RichText::new(&app.vcf_status).size(btn_size));
                }
            });
            ui.add_space(4.0);
        });
    }
