// 画面上部のツールバー（ファイル操作、編集、保存、リセット）の描画を行う
use crate::ExportState;
use crate::core::NO_NODE;
use crate::lang;
use eframe::egui;

/// Draws the VCF/VCT solution navigation.  On narrow touch screens this is
/// deliberately kept as one unit on its own row, so the controls do not get
/// split by the search buttons or status text.
fn draw_vcf_replay_controls(
    ui: &mut egui::Ui,
    app: &mut crate::RenjuApp,
    tr: &lang::Tr,
    btn_size: f32,
) {
    if app.vcf_solving || app.vcf_solution.is_empty() {
        return;
    }

    if ui
        .add_enabled(
            app.vcf_replay_len > 0,
            egui::Button::new(egui::RichText::new("⏮").size(btn_size)),
        )
        .on_hover_text(tr.vcf_replay_start)
        .clicked()
    {
        app.vcf_replay_to_start();
    }
    if ui
        .add_enabled(
            app.vcf_replay_len > 0,
            egui::Button::new(egui::RichText::new("◀").size(btn_size)),
        )
        .on_hover_text(tr.vcf_replay_prev)
        .clicked()
    {
        app.vcf_replay_prev();
    }
    ui.label(
        egui::RichText::new(format!("{}/{}", app.vcf_replay_len, app.vcf_solution.len()))
            .size(btn_size),
    );
    if ui
        .add_enabled(
            app.vcf_replay_len < app.vcf_solution.len(),
            egui::Button::new(egui::RichText::new("▶").size(btn_size)),
        )
        .on_hover_text(tr.vcf_replay_next)
        .clicked()
    {
        app.vcf_replay_next();
    }
    if ui
        .add_enabled(
            app.vcf_replay_len < app.vcf_solution.len(),
            egui::Button::new(egui::RichText::new("⏭").size(btn_size)),
        )
        .on_hover_text(tr.vcf_replay_end)
        .clicked()
    {
        app.vcf_replay_to_end();
    }
}

/// Draws the normal game-record navigation controls (not the VCF/VCT replay).
fn draw_history_navigation_controls(
    ui: &mut egui::Ui,
    app: &mut crate::RenjuApp,
    tr: &lang::Tr,
    btn_size: f32,
) {
    if ui
        .button(egui::RichText::new(tr.undo_all).size(btn_size))
        .on_hover_text(tr.undo_all_tooltip)
        .clicked()
    {
        app.reset_to_root();
    }

    if ui
        .button(egui::RichText::new(tr.undo_one).size(btn_size))
        .on_hover_text(tr.undo_one_tooltip)
        .clicked()
    {
        let before_len = app.board.history.len();
        if app.board.custom_position || app.is_db_mode {
            app.board.undo();
        } else if let Some(nodes) = &app.lib_nodes {
            if let Some(idx) = app.current_node_idx {
                let node = &nodes[idx];
                let p_idx = node.parent;
                if p_idx != NO_NODE {
                    if node.x() >= 0 && node.y() >= 0 {
                        app.board.undo();
                    }
                    app.current_node_idx = Some(p_idx as usize);
                }
            }
        } else {
            app.board.undo();
        }
        if app.board.history.len() != before_len {
            app.clear_vcf_solution();
        }
    }

    if ui
        .button(egui::RichText::new(tr.redo_one).size(btn_size))
        .on_hover_text(tr.redo_one_tooltip)
        .clicked()
    {
        app.step_forward();
    }

    if ui
        .button(egui::RichText::new(tr.redo_all).size(btn_size))
        .on_hover_text(tr.redo_all_tooltip)
        .clicked()
    {
        while app.step_forward() {}
    }
}

pub fn draw_toolbar(app: &mut crate::RenjuApp, ctx: &egui::Context, tr: &lang::Tr) {
    egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
        #[cfg(target_os = "android")]
        {
            ui.add_space(app.android_safe_inset_top_points(ctx));
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let (btn_size, spacing) = if cfg!(target_arch = "wasm32") {
                (
                    app.settings.wasm_button_size,
                    app.settings.wasm_button_size * 0.5,
                )
            } else if cfg!(target_os = "android") {
                (
                    app.settings.android_button_size,
                    app.settings.android_button_size * 0.5,
                )
            } else {
                (20.0, 12.0)
            };

            ui.spacing_mut().item_spacing.x = spacing;
            if ui
                .button(egui::RichText::new(tr.new_game).size(btn_size))
                .on_hover_text(tr.new_game_tooltip)
                .clicked()
            {
                app.show_new_game_confirm = true;
            }

            if ui
                .button(egui::RichText::new(tr.settings).size(btn_size))
                .on_hover_text(tr.settings_tooltip)
                .clicked()
            {
                app.settings.show_window = !app.settings.show_window;
            }

            if ui
                .button(egui::RichText::new(tr.open_file).size(btn_size))
                .on_hover_text(tr.open_file_tooltip)
                .clicked()
            {
                #[cfg(all(
                    not(target_arch = "wasm32"),
                    not(target_os = "android"),
                    not(target_os = "ios")
                ))]
                {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Renju Files", &["lib", "db"])
                        .pick_file()
                    {
                        app.load_file(&path);
                    }
                }

                #[cfg(target_os = "android")]
                {
                    app.request_android_open_file();
                }

                #[cfg(target_os = "ios")]
                {
                    app.request_ios_open_file();
                }

                #[cfg(target_arch = "wasm32")]
                {
                    let dropped_file_clone = app.wasm_dropped_file.clone();
                    let ctx_clone = ctx.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        if let Some(file) = rfd::AsyncFileDialog::new()
                            .add_filter("Renju", &["lib", "db"])
                            .pick_file()
                            .await
                        {
                            let file_name = file.file_name();
                            let data = file.read().await;
                            if let Ok(mut lock) = dropped_file_clone.lock() {
                                *lock = Some((file_name, data));
                            }
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
            })
            .response
            .on_hover_text(tr.save_as_tooltip);

            ui.separator();

            if !(cfg!(target_arch = "wasm32") || cfg!(target_os = "android")) {
                draw_history_navigation_controls(ui, app, tr, btn_size);
            }

            ui.separator();

            if ui
                .button(egui::RichText::new("🗑").size(btn_size))
                .on_hover_text(tr.delete_confirm_title)
                .clicked()
            {
                if app.node_to_delete.is_none() && app.editing_coord.is_none() {
                    if let Some(idx) = app.get_current_node() {
                        if idx > 0 {
                            app.node_to_delete = Some(idx);
                        }
                    }
                }
            }

            if ui
                .toggle_value(
                    &mut app.is_text_mode,
                    egui::RichText::new("📝").size(btn_size),
                )
                .on_hover_text("Text Edit Mode")
                .clicked()
            {
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
            })
            .response
            .on_hover_text("画像として保存");

            if ui
                .button(egui::RichText::new("🎬").size(btn_size))
                .on_hover_text("GIFアニメとして保存")
                .clicked()
            {
                app.show_gif_setup = true;
                app.gif_settings.end_move = app.board.history.len();
            }

            if ui
                .button(egui::RichText::new("ℹ️").size(btn_size))
                .on_hover_text("About")
                .clicked()
            {
                app.show_about = true;
            }
        });
        ui.add_space(2.0);
        // Keep the mobile search and normal game-record controls on one
        // fixed row. VCF/VCT replay is rendered below it to keep the toolbar
        // height stable while the replay position changes.
        let touch_target = cfg!(target_arch = "wasm32") || cfg!(target_os = "android");
        let replay_btn_size = if cfg!(target_arch = "wasm32") {
            app.settings.wasm_button_size * 0.8
        } else if cfg!(target_os = "android") {
            app.settings.android_button_size * 0.8
        } else {
            16.0
        };
        let mut draw_vcf_controls = |ui: &mut egui::Ui| {
            let (btn_size, spacing) = if cfg!(target_arch = "wasm32") {
                (
                    app.settings.wasm_button_size * 0.8,
                    app.settings.wasm_button_size * 0.35,
                )
            } else if cfg!(target_os = "android") {
                (
                    app.settings.android_button_size * 0.8,
                    app.settings.android_button_size * 0.35,
                )
            } else {
                (16.0, 8.0)
            };

            ui.spacing_mut().item_spacing.x = spacing;
            if ui
                .add_enabled(
                    !app.vcf_solving,
                    egui::Button::new(egui::RichText::new(tr.vcf).size(btn_size).strong()),
                )
                .clicked()
            {
                app.start_vcf_search();
            }
            if ui
                .add_enabled(
                    !app.vcf_solving,
                    egui::Button::new(egui::RichText::new(tr.vct).size(btn_size).strong()),
                )
                .clicked()
            {
                app.start_vct_search();
            }
            if ui
                .add_enabled(
                    !app.vcf_solving,
                    egui::Button::new(egui::RichText::new(tr.vcf_threat).size(btn_size).strong()),
                )
                .clicked()
            {
                app.start_vcf_threat_search();
            }
            if touch_target {
                draw_history_navigation_controls(ui, app, tr, btn_size);
            }
            #[cfg(target_os = "windows")]
            if ui
                .button(egui::RichText::new("Log").size(btn_size).strong())
                .on_hover_text("Show VCT search log")
                .clicked()
            {
                app.show_vct_log = true;
            }

            if app.vcf_solving {
                if ui
                    .button(egui::RichText::new("⏹").size(btn_size))
                    .on_hover_text(tr.vcf_cancel)
                    .clicked()
                {
                    app.cancel_vcf_search();
                }
                ui.spinner();
                let solving_text = if let Some(elapsed) = app.current_vcf_elapsed_secs() {
                    format!("{} {:.1}s", tr.vcf_solving, elapsed)
                } else {
                    tr.vcf_solving.to_string()
                };
                ui.label(egui::RichText::new(solving_text).size(btn_size));
            } else if !app.vcf_status.is_empty() {
                ui.label(egui::RichText::new(&app.vcf_status).size(btn_size));
            }

            if !touch_target {
                draw_vcf_replay_controls(ui, app, tr, btn_size);
            }
        };
        ui.horizontal(|ui| {
            draw_vcf_controls(ui);
        });
        if touch_target && !app.vcf_solving && !app.vcf_solution.is_empty() {
            ui.add_space(2.0);
            let replay_height = replay_btn_size + ui.spacing().button_padding.y * 2.0 + 4.0;
            // Reserve a full-width, fixed-height row. This must not share the
            // action row: otherwise egui's available-width calculation can
            // occasionally keep the replay controls on that row on touch UIs.
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), replay_height),
                egui::Layout::left_to_right(egui::Align::Min),
                |ui| draw_vcf_replay_controls(ui, app, tr, replay_btn_size),
            );
        }
        ui.add_space(4.0);
    });
}
