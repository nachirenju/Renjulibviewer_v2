// 設定、バージョン情報、削除確認などの各種ダイアログの描画を行う
use eframe::egui;
use crate::lang;
use crate::ExportState;
use crate::core::{HashBoard, RenLibNode, NO_NODE, NO_TEXT};
pub fn draw_about_dialog(app: &mut crate::RenjuApp, ctx: &egui::Context) {
        if app.show_about {
                let mut open = app.show_about;
                egui::Window::new("About Renjulibviewer")
                    .open(&mut open)
                    .collapsible(false)
                    .resizable(true)
                    .default_width(450.0)
                    .show(ctx, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            ui.heading("Renjulibviewer");
                            ui.add_space(8.0);
                            
                            ui.label("本アプリは、連珠の.lib及び.dbファイルのビューアーです。");
                            ui.label("現時点ではlibの閲覧や保存に最適化されています。");
                            ui.label("本アプリを作成するにあたってRapfiというソフトウェアの読み込み部分を参考にしました。");
                            
                            ui.add_space(4.0);
                            ui.hyperlink("https://github.com/dhbloo/rapfi");
                            
                            ui.add_space(16.0);
                            ui.separator();
                            ui.add_space(8.0);

                            ui.heading("License Information");
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new("GNU General Public License version 3").strong());
                            ui.add_space(4.0);
                            
                            let gpl_text1 = "Rapfi is free and distributed under the GNU General Public License version 3 (GPL v3). Essentially, this means you are free to do almost exactly what you want with the program, including distributing it among your friends, making it available for download from your website, selling it (either by itself or as part of some bigger software package), or using it as the starting point for a software project of your own.";
                            let gpl_text2 = "The only real limitation is that whenever you distribute Rapfi in some way, you MUST always include the license and the full source code (or a pointer to where the source code can be found) to generate the exact binary you are distributing. If you make any changes to the source code, these changes must also be made available under GPL v3.";
                            
                            ui.label(gpl_text1);
                            ui.add_space(8.0);
                            ui.label(gpl_text2);

                            ui.add_space(16.0);
                            ui.separator();
                            ui.add_space(8.0);

                            ui.add_space(4.0);
                            ui.label("製作者: nachirenju");
                            ui.horizontal(|ui| {
                                ui.label("GitHub:");
                                ui.hyperlink_to("Renjulibviewer_v2", "https://github.com/nachirenju/Renjulibviewer_v2");
                            });
                            ui.horizontal(|ui| {
                                ui.label("X (Twitter):");
                                ui.hyperlink_to("@nachirenju", "https://x.com/nachirenju");
                            });
                            ui.add_space(8.0);
                        });
                    });
                app.show_about = open;
            }
    }

pub fn draw_gif_setup_dialog(app: &mut crate::RenjuApp, ctx: &egui::Context) {
        if app.show_gif_setup {
                let mut open = app.show_gif_setup;
                let mut close_requested = false;
                egui::Window::new("GIF保存詳細設定").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("開始手数: ");
                        ui.add(egui::DragValue::new(&mut app.gif_settings.start_move).range(0..=app.board.history.len()));
                    });
                    ui.horizontal(|ui| {
                        ui.label("終了手数: ");
                        let max_moves = app.board.history.len();
                        ui.add(egui::DragValue::new(&mut app.gif_settings.end_move).range(app.gif_settings.start_move..=max_moves));
                    });
                    ui.horizontal(|ui| {
                        ui.label("石番号表示を開始させる手数: ");
                        ui.add(egui::DragValue::new(&mut app.gif_settings.show_number_start_move).range(1..=app.board.history.len().max(1)));
                    });
                    ui.horizontal(|ui| {
                        ui.label("石番号表示における開始番号: ");
                        ui.add(egui::DragValue::new(&mut app.gif_settings.show_number_start_value).range(1..=1000));
                    });
                    ui.horizontal(|ui| {
                        ui.label("更新間隔(秒): ");
                        ui.add(egui::DragValue::new(&mut app.gif_settings.frame_delay_sec).speed(0.1).range(0.1..=10.0));
                    });
                    ui.horizontal(|ui| {
                        ui.label("最初フレーム静止(秒): ");
                        ui.add(egui::DragValue::new(&mut app.gif_settings.first_frame_delay_sec).speed(0.1).range(0.1..=10.0));
                    });
                    ui.horizontal(|ui| {
                        ui.label("最終フレーム静止(秒): ");
                        ui.add(egui::DragValue::new(&mut app.gif_settings.last_frame_delay_sec).speed(0.1).range(0.1..=10.0));
                    });
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("盤面全体を保存").clicked() {
                            app.export_state = ExportState::GifRequestFull;
                            app.gif_frames.clear();
                            close_requested = true;
                        }
                        if ui.button("範囲を指定して保存").clicked() {
                            app.export_state = ExportState::GifSelectingRegion(None, None);
                            app.gif_frames.clear();
                            close_requested = true;
                        }
                    });
                });
                app.show_gif_setup = open && !close_requested;
            }
    }

pub fn draw_delete_confirm_dialog(app: &mut crate::RenjuApp, ctx: &egui::Context, tr: &lang::Tr) {
        if app.node_to_delete.is_some() {
            egui::Window::new(tr.delete_confirm_title)
                .collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.label(tr.delete_confirm_msg);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(tr.delete_yes).clicked() { app.delete_current_branch(); }
                        if ui.button(tr.delete_no).clicked() { app.node_to_delete = None; }
                    });
                });
        }
    }

pub fn draw_text_edit_dialog(app: &mut crate::RenjuApp, ctx: &egui::Context) {
        if let Some(coord) = app.editing_coord {
            egui::Window::new("Text Edit")
                .collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.label(format!("Coordinate: {}{}", (b'A' + coord.0 as u8) as char, 15 - coord.1));
                    let response = ui.text_edit_singleline(&mut app.editing_text);
                    if !response.has_focus() { response.request_focus(); }

                    let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));

                    let mut width = 0;
                    let mut limited_text = String::new();
                    for c in app.editing_text.chars() {
                        let c_width = if c.is_ascii() { 1 } else { 2 };
                        if width + c_width <= 4 { limited_text.push(c); width += c_width; } else { break; }
                    }
                    app.editing_text = limited_text;

                    ui.horizontal(|ui| {
                        if ui.button("OK").clicked() || enter_pressed {
                            let new_hash = {
                                let mut pb = HashBoard::new();
                                for &m in &app.board.history { pb.make_move((m % 15) as i8, (m / 15) as i8); }
                                pb.make_move(coord.0, coord.1);
                                pb.get_canonical_hash()
                            };

                            let current_idx = app.get_current_node();

                            if let Some(nodes) = &mut app.lib_nodes {
                                let mut direct_child_idx = None;
                                if let Some(idx) = current_idx {
                                    let mut c = nodes[idx].child;
                                    while c != NO_NODE {
                                        if nodes[c as usize].x() == coord.0 && nodes[c as usize].y() == coord.1 {
                                            direct_child_idx = Some(c as usize); break;
                                        }
                                        c = nodes[c as usize].sibling;
                                    }
                                }

                                let target_idx = if let Some(c_idx) = direct_child_idx {
                                    c_idx
                                } else if let Some(idx) = current_idx {
                                    let new_idx = nodes.len();
                                    nodes.push(RenLibNode::new(
                                        coord.0, coord.1, idx as u32, NO_NODE, nodes[idx].child, new_hash, nodes[idx].depth() + 1
                                    ));
                                    nodes[idx].child = new_idx as u32; 
                                    
                                    if let Some(ht) = &mut app.hash_table {
                                        ht.insert(new_hash, new_idx as u32);
                                    }
                                    new_idx
                                } else { 0 };

                                let encoding = app.settings.text_encoding.to_encoding_rs();
                                let new_text_id = if app.editing_text.is_empty() { NO_TEXT } else { 
                                    let bytes = encoding.encode(&app.editing_text).0.into_owned();
                                    app.text_pool.as_mut().unwrap().get_or_insert(&bytes)
                                };

                                if let Some(ht) = &app.hash_table {
                                    if let Some(&head) = ht.get(&new_hash) {
                                        app.set_text_id(head as usize, new_text_id);
                                    }
                                }
                                app.set_text_id(target_idx, new_text_id);
                                app.subtree_cache.borrow_mut().clear();
                            }
                            app.editing_coord = None;
                        }
                        
                        if ui.button("Cancel").clicked() || (ui.input(|i| i.key_pressed(egui::Key::Escape))) {
                            app.editing_coord = None;
                        }
                    });
                });
        }
    }