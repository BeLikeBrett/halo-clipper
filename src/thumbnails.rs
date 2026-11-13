//! Thumbnail extraction stays off the UI thread and never changes the timeline.
use super::*;
use std::sync::mpsc;
#[derive(Default)]
pub(super) struct Thumbnails {
    images: HashMap<PathBuf, Option<egui::TextureHandle>>,
    sender: Option<mpsc::SyncSender<PathBuf>>,
    receiver: Option<mpsc::Receiver<(PathBuf, Option<egui::ColorImage>)>>,
}
impl Thumbnails {
    pub fn get(
        &mut self,
        ctx: &egui::Context,
        path: &std::path::Path,
    ) -> Option<egui::TextureHandle> {
        if self.sender.is_none() {
            let (tx, rx) = mpsc::sync_channel::<PathBuf>(16);
            let (results, out) = mpsc::channel();
            let context = ctx.clone();
            std::thread::spawn(move || {
                while let Ok(path) = rx.recv() {
                    let image = extract(&path)
                        .map_err(|e| log::warn!("thumbnail: {e:#}"))
                        .ok();
                    let _ = results.send((path, image));
                    context.request_repaint();
                }
            });
            self.sender = Some(tx);
            self.receiver = Some(out);
        }
        if let Some(rx) = &self.receiver {
            while let Ok((path, image)) = rx.try_recv() {
                let image = image.map(|image| {
                    ctx.load_texture("media-thumbnail", image, egui::TextureOptions::LINEAR)
                });
                self.images.insert(path, image);
            }
        }
        if !self.images.contains_key(path)
            && self
                .sender
                .as_ref()
                .unwrap()
                .try_send(path.to_path_buf())
                .is_ok()
        {
            self.images.insert(path.to_path_buf(), None);
        }
        self.images.get(path).and_then(Clone::clone)
    }
}
fn extract(path: &std::path::Path) -> anyhow::Result<egui::ColorImage> {
    let output = crate::platform::command("ffmpeg")
        .args(["-v", "error", "-nostdin", "-threads", "2", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:v:0",
            "-frames:v",
            "1",
            "-vf",
            "scale=240:135:force_original_aspect_ratio=decrease,pad=240:135:(ow-iw)/2:(oh-ih)/2",
            "-f",
            "image2pipe",
            "-vcodec",
            "png",
            "-",
        ])
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "Could not decode a media thumbnail"
    );
    let image = image::load_from_memory(&output.stdout)?.into_rgba8();
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extracts_a_real_video_frame() {
        let path = std::env::temp_dir().join(format!("halo-thumb-{}.mp4", std::process::id()));
        let status = crate::platform::command("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=red:s=64x64:d=0.1",
                "-c:v",
                "libx264",
            ])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        let image = extract(&path).unwrap();
        assert_eq!(image.size, [240, 135]);
        let pixel = image.pixels[67 * 240 + 120];
        assert!(pixel.r() > 200 && pixel.b() < 30);
        let _ = std::fs::remove_file(path);
    }
}
