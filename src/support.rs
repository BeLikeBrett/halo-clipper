use super::*;
impl ClipperApp {
    pub(super) fn show_support(&mut self, ctx: &egui::Context) {
        self.updater.poll();
        if matches!(self.updater.state, crate::updater::State::Working(_)) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let mut open = self.support_open;
        egui::Window::new("Updates & support")
            .open(&mut open)
            .default_width(460.0)
            .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .collapsible(false)
            .resizable(true)
            .show(ctx, |ui| {
                // ── Header: name, version, and a one-word status ──
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Halo Clipper")
                            .font(theme::semibold(18.0))
                            .color(theme::TEXT_PRIMARY),
                    );
                    ui.label(
                        RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                            .font(theme::mono(13.0))
                            .color(theme::TEXT_MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let status = match &self.updater.state {
                            crate::updater::State::Available(..) => Some(("Update available", theme::ACCENT)),
                            crate::updater::State::Staged(_) => Some(("Ready to install", theme::ACCENT)),
                            crate::updater::State::Current => Some(("Up to date", theme::TEXT_MUTED)),
                            _ => None,
                        };
                        if let Some((text, color)) = status {
                            ui.label(RichText::new(text).size(12.0).color(color));
                        }
                    });
                });
                ui.add_space(2.0);
                ui.hyperlink_to(
                    RichText::new("Project & releases on GitHub").size(12.5),
                    crate::updater::REPO,
                );
                ui.add_space(12.0);

                if ui
                    .checkbox(
                        &mut self.settings.check_updates_on_startup,
                        "Check for updates when Halo Clipper starts",
                    )
                    .changed()
                {
                    if let Err(e) = self.settings.save() {
                        self.status = format!("Could not save update preference: {e:#}");
                    }
                }
                ui.add_space(8.0);

                // ── Update status card ──
                let mut check = false;
                let mut download = None;
                let mut install = None;
                egui::Frame::none()
                    .fill(theme::BG_BUTTON)
                    .rounding(egui::Rounding::same(8.0))
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER_SUBTLE))
                    .inner_margin(egui::Margin::symmetric(14.0, 12.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        match &self.updater.state {
                            crate::updater::State::Idle => {
                                ui.label(
                                    RichText::new("Check whether a newer release is available.")
                                        .color(theme::TEXT_SECONDARY),
                                );
                                ui.add_space(8.0);
                                check = ui.button("Check for updates").clicked();
                            }
                            crate::updater::State::Working(message) => {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(RichText::new(message).color(theme::TEXT_SECONDARY));
                                });
                            }
                            crate::updater::State::Current => {
                                ui.label(
                                    RichText::new("You’re on the latest stable release.")
                                        .color(theme::TEXT_SECONDARY),
                                );
                                ui.add_space(8.0);
                                check = ui.button("Check again").clicked();
                            }
                            crate::updater::State::Available(m, notes) => {
                                ui.label(
                                    RichText::new(format!("Version {} is available", m.version))
                                        .font(theme::medium(14.5))
                                        .color(theme::TEXT_PRIMARY),
                                );
                                ui.add_space(8.0);
                                match notes.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                                    Some(text) => {
                                        ui.label(
                                            RichText::new("What’s new")
                                                .size(11.5)
                                                .color(theme::TEXT_MUTED),
                                        );
                                        ui.add_space(3.0);
                                        egui::ScrollArea::vertical()
                                            .id_salt("update_notes")
                                            .max_height(150.0)
                                            .auto_shrink([false, true])
                                            .show(ui, |ui| {
                                                ui.label(
                                                    RichText::new(text)
                                                        .size(12.5)
                                                        .color(theme::TEXT_SECONDARY),
                                                );
                                            });
                                    }
                                    None => {
                                        ui.hyperlink_to(
                                            "Read the release notes on GitHub",
                                            format!(
                                                "{}/releases/tag/v{}",
                                                crate::updater::REPO,
                                                m.version
                                            ),
                                        );
                                    }
                                }
                                ui.add_space(12.0);
                                ui.horizontal(|ui| {
                                    if ui.button("Download & verify update").clicked() {
                                        download = Some(m.clone());
                                    }
                                    ui.hyperlink_to(
                                        RichText::new("Release page").size(12.5),
                                        format!(
                                            "{}/releases/tag/v{}",
                                            crate::updater::REPO,
                                            m.version
                                        ),
                                    );
                                });
                            }
                            crate::updater::State::Staged(path) => {
                                ui.label(
                                    RichText::new("Update downloaded — signature and checksum verified.")
                                        .color(theme::TEXT_SECONDARY),
                                );
                                ui.add_space(4.0);
                                ui.label(
                                    RichText::new(
                                        "Installing closes and restarts Halo Clipper. Export your \
                                         work first — the timeline isn’t saved.",
                                    )
                                    .size(12.0)
                                    .color(theme::TEXT_MUTED),
                                );
                                ui.add_space(10.0);
                                let busy = self.export.is_some();
                                if ui
                                    .add_enabled(!busy, egui::Button::new("Install update & restart"))
                                    .clicked()
                                {
                                    install = Some(path.clone());
                                }
                                if busy {
                                    ui.add_space(4.0);
                                    ui.label(
                                        RichText::new("Close the export dialog before installing.")
                                            .size(12.0)
                                            .color(theme::ACCENT_ERR),
                                    );
                                }
                            }
                            crate::updater::State::Error(e) => {
                                ui.colored_label(theme::ACCENT_ERR, e);
                                ui.add_space(8.0);
                                check = ui.button("Try again").clicked();
                            }
                        }
                    });

                if check {
                    self.updater.check();
                }
                if let Some(m) = download {
                    self.updater.download(m);
                }
                if let Some(path) = install {
                    match crate::updater::install(&path) {
                        Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(e) => {
                            self.updater.state = crate::updater::State::Error(format!("{e:#}"))
                        }
                    }
                }

                ui.add_space(16.0);
                ui.separator();
                ui.add_space(10.0);

                // ── Report a problem ──
                ui.label(
                    RichText::new("Report a problem")
                        .font(theme::medium(14.5))
                        .color(theme::TEXT_PRIMARY),
                );
                ui.add_space(3.0);
                ui.label(
                    RichText::new(
                        "Logs stay on this device. Export a report, look it over, then attach it \
                         to a GitHub issue — sharing is your choice.",
                    )
                    .size(12.0)
                    .color(theme::TEXT_MUTED),
                );
                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Export support report…").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_file_name("halo-clipper-support.txt")
                            .save_file()
                        {
                            let context = if let Some(p) = &self.project {
                                format!(
                                    "Tracks: {}\nClips: {}\nSource: {}x{} {:.2}fps\nAudio streams: {}\nGPU: {:?}\nEncoders: {:?}",
                                    p.tracks.len(),
                                    p.clips.len(),
                                    p.media.width,
                                    p.media.height,
                                    p.media.fps,
                                    p.media.audio_stream_count,
                                    self.detected_gpu,
                                    self.available_codecs
                                )
                            } else {
                                "No media loaded".into()
                            };
                            self.status = match crate::diagnostics::export(&path, &context) {
                                Ok(()) => "Support report saved. Review it before sharing.".into(),
                                Err(e) => format!("Could not save report: {e:#}"),
                            };
                        }
                    }
                    ui.hyperlink_to(
                        "Open bug report",
                        format!("{}/issues/new?template=bug_report.yml", crate::updater::REPO),
                    );
                    if ui.button("Open log folder").clicked() {
                        let _ = crate::platform::command(if cfg!(windows) {
                            "explorer.exe"
                        } else {
                            "xdg-open"
                        })
                        .arg(crate::diagnostics::log_dir())
                        .spawn();
                    }
                });
            });
        self.support_open = open;
    }
}
