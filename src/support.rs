use super::*;
impl ClipperApp {
    pub(super) fn show_support(&mut self, ctx: &egui::Context) {
        self.updater.poll();
        if matches!(self.updater.state, crate::updater::State::Working(_)) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let mut open = self.support_open;
        egui::Window::new("Updates & support").open(&mut open).default_width(480.0).anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO).collapsible(false).resizable(true).show(ctx,|ui| {
            ui.label(RichText::new(format!("Halo Clipper {}",env!("CARGO_PKG_VERSION"))).strong().size(18.0));
            ui.hyperlink_to("Project & releases",crate::updater::REPO);
            if ui.checkbox(&mut self.settings.check_updates_on_startup, "Check for updates on startup (GitHub)").changed() {
                if let Err(e)=self.settings.save() { self.status=format!("Could not save update preference: {e:#}"); }
            }
            ui.separator();
            let state=&self.updater.state;
            let mut check=false;let mut download=None;let mut install=None;
            match state {
                crate::updater::State::Idle=>{ui.label("Check for the latest stable release.");check=ui.button("Check for updates").clicked();},
                crate::updater::State::Working(message)=>{ui.spinner();ui.label(message);},
                crate::updater::State::Current=>{ui.label("You’re running the latest stable release.");check=ui.button("Check again").clicked();},
                crate::updater::State::Available(m)=>{
                    ui.label(format!("Version {} is available.",m.version));
                    ui.hyperlink_to("Read release notes",format!("{}/releases/tag/v{}",crate::updater::REPO,m.version));
                    if ui.button("Download verified update").clicked() {download=Some(m.clone());}
                },
                crate::updater::State::Staged(path)=>{
                    ui.label("Update downloaded. Signature and checksum verified.");
                    ui.label("Installing closes and restarts Halo Clipper. Export your work first; timeline sessions are not saved.");
                    let busy=self.export.is_some();
                    if ui.add_enabled(!busy,egui::Button::new("Install update & restart")).clicked(){install=Some(path.clone());}
                    if busy {ui.label("Close the export dialog before installing.");}
                },
                crate::updater::State::Error(e)=>{ui.colored_label(theme::ACCENT_ERR,e);check=ui.button("Try again").clicked();},
            }
            if check {self.updater.check();}
            if let Some(m)=download {self.updater.download(m);}
            if let Some(path)=install {match crate::updater::install(&path) {
                Ok(())=>ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Err(e)=>self.updater.state=crate::updater::State::Error(format!("{e:#}")),
            }}
            ui.add_space(12.0);ui.separator();ui.label(RichText::new("Report a problem").strong());
            ui.label("Logs stay on this device. Export a support report, review it, then attach it to a GitHub issue. Sharing is your choice.");
            ui.horizontal_wrapped(|ui| {
                if ui.button("Export support report…").clicked() {
                    if let Some(path)=rfd::FileDialog::new().set_file_name("halo-clipper-support.txt").save_file() {
                        let context=if let Some(p)=&self.project {format!("Tracks: {}\nClips: {}\nSource: {}x{} {:.2}fps\nAudio streams: {}\nGPU: {:?}\nEncoders: {:?}",p.tracks.len(),p.clips.len(),p.media.width,p.media.height,p.media.fps,p.media.audio_stream_count,self.detected_gpu,self.available_codecs)} else {"No media loaded".into()};
                        self.status=match crate::diagnostics::export(&path,&context){Ok(())=>"Support report saved. Review it before sharing.".into(),Err(e)=>format!("Could not save report: {e:#}")};
                    }
                }
                ui.hyperlink_to("Open bug report",format!("{}/issues/new?template=bug_report.yml",crate::updater::REPO));
                if ui.button("Open log folder").clicked(){let _=crate::platform::command(if cfg!(windows) {"explorer.exe"} else {"xdg-open"}).arg(crate::diagnostics::log_dir()).spawn();}
            });
        });
        self.support_open = open;
    }
}
