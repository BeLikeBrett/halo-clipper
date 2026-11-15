//! File drops on native Wayland.
//!
//! winit 0.30 only reports dropped files on X11; on a Wayland session
//! (niri, GNOME, KDE…) a drag from a file manager never reaches the app.
//! This listens on winit's own `wl_display` with a private event queue: it
//! binds `wl_data_device_manager`, gets a data device for the seat, accepts
//! `text/uri-list` offers and reads them on drop. As a bonus the protocol
//! reports the pointer position during the drag, which winit doesn't.
//!
//! The queue is dispatched on a small thread that polls with a timeout, so
//! [`WaylandDrops::stop`] can end it before the display is closed.

use std::collections::HashMap;
use std::io::Read;
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use wayland_client::backend::Backend;
use wayland_client::protocol::{
    wl_data_device, wl_data_device_manager, wl_data_offer, wl_registry, wl_seat,
};
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle};

const URI_LIST: &str = "text/uri-list";

/// What the UI reads each frame.
#[derive(Default)]
struct Shared {
    /// Surface-local pointer position while files hover the window.
    hover: Option<(f64, f64)>,
    /// Files dropped since the last [`WaylandDrops::take`].
    dropped: Vec<PathBuf>,
}

/// A running listener. Dropping it stops the thread.
pub struct WaylandDrops {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WaylandDrops {
    /// Starts listening on the `wl_display` winit created.
    ///
    /// # Safety
    /// `display` must be a live `wl_display` that outlives this listener
    /// (call [`WaylandDrops::stop`] before the window is destroyed).
    pub unsafe fn start(display: *mut std::ffi::c_void, repaint: egui::Context) -> Option<Self> {
        // SAFETY: guaranteed by the caller.
        let backend = unsafe { Backend::from_foreign_display(display.cast()) };
        let conn = Connection::from_backend(backend);
        let mut queue = conn.new_event_queue::<State>();
        let qh = queue.handle();
        conn.display().get_registry(&qh, ());
        let shared = Arc::new(Mutex::new(Shared::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let mut state = State {
            conn: conn.clone(),
            shared: shared.clone(),
            repaint,
            seat: None,
            manager: None,
            device: None,
            mimes: HashMap::new(),
            current: None,
        };
        let halt = stop.clone();
        let thread = std::thread::Builder::new()
            .name("wayland-drops".into())
            .spawn(move || {
                while !halt.load(Ordering::Relaxed) {
                    if queue.dispatch_pending(&mut state).is_err() || conn.flush().is_err() {
                        break;
                    }
                    let Some(guard) = queue.prepare_read() else {
                        continue;
                    };
                    // 100 ms keeps shutdown prompt without busy-waiting.
                    match wait_readable(guard.connection_fd(), 100) {
                        Ok(true) => {
                            // An error here is a dead connection; the
                            // next dispatch reports it and ends the loop.
                            let _ = guard.read();
                        }
                        Ok(false) => drop(guard),
                        Err(_) => break,
                    }
                }
            })
            .ok()?;
        Some(Self {
            shared,
            stop,
            thread: Some(thread),
        })
    }

    /// Pointer position (surface-local, logical px) while files hover.
    pub fn hover(&self) -> Option<(f64, f64)> {
        self.shared.lock().ok()?.hover
    }

    /// Files dropped since the last call.
    pub fn take(&self) -> Vec<PathBuf> {
        self.shared
            .lock()
            .map(|mut s| std::mem::take(&mut s.dropped))
            .unwrap_or_default()
    }

    /// Ends the listener thread (before the display goes away).
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for WaylandDrops {
    fn drop(&mut self) {
        self.stop();
    }
}

struct State {
    conn: Connection,
    shared: Arc<Mutex<Shared>>,
    repaint: egui::Context,
    seat: Option<wl_seat::WlSeat>,
    manager: Option<wl_data_device_manager::WlDataDeviceManager>,
    device: Option<wl_data_device::WlDataDevice>,
    /// MIME types announced per offer.
    mimes: HashMap<wayland_client::backend::ObjectId, Vec<String>>,
    /// The offer of the drag over the window, if it carries files.
    current: Option<wl_data_offer::WlDataOffer>,
}

impl State {
    fn set_hover(&self, hover: Option<(f64, f64)>) {
        if let Ok(mut s) = self.shared.lock() {
            s.hover = hover;
        }
        self.repaint.request_repaint();
    }

    fn ensure_device(&mut self, qh: &QueueHandle<Self>) {
        if self.device.is_none() {
            if let (Some(manager), Some(seat)) = (&self.manager, &self.seat) {
                self.device = Some(manager.get_data_device(seat, qh, ()));
            }
        }
    }

    fn forget(&mut self, offer: &wl_data_offer::WlDataOffer) {
        self.mimes.remove(&offer.id());
        offer.destroy();
    }

    /// Reads the dropped URI list off the UI's way, then finishes the offer.
    fn receive(&mut self, offer: wl_data_offer::WlDataOffer) {
        self.mimes.remove(&offer.id());
        let Ok((reader, writer)) = std::io::pipe() else {
            offer.destroy();
            return;
        };
        offer.receive(URI_LIST.into(), writer.as_fd());
        drop(writer);
        let _ = self.conn.flush();
        let (conn, shared, repaint) =
            (self.conn.clone(), self.shared.clone(), self.repaint.clone());
        // The source writes at its own pace; never block the queue on it.
        let _ = std::thread::Builder::new()
            .name("wayland-drop-read".into())
            .spawn(move || {
                let mut data = Vec::new();
                let _ = reader.take(1 << 20).read_to_end(&mut data);
                if offer.version() >= 3 {
                    offer.finish();
                }
                offer.destroy();
                let _ = conn.flush();
                let paths = parse_uri_list(&data);
                if let Ok(mut s) = shared.lock() {
                    s.dropped.extend(paths);
                    s.hover = None;
                }
                repaint.request_repaint();
            });
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        match interface.as_str() {
            "wl_seat" if state.seat.is_none() => {
                state.seat = Some(registry.bind(name, version.min(5), qh, ()));
            }
            "wl_data_device_manager" if state.manager.is_none() => {
                state.manager = Some(registry.bind(name, version.min(3), qh, ()));
            }
            _ => return,
        }
        state.ensure_device(qh);
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_data_device_manager::WlDataDeviceManager, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_data_device_manager::WlDataDeviceManager,
        _: wl_data_device_manager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_data_offer::WlDataOffer, ()> for State {
    fn event(
        state: &mut Self,
        offer: &wl_data_offer::WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            state.mimes.entry(offer.id()).or_default().push(mime_type);
        }
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wl_data_device::Event;
        match event {
            Event::DataOffer { .. } => {}
            Event::Enter {
                serial, x, y, id, ..
            } => {
                if let Some(old) = state.current.take() {
                    state.forget(&old);
                }
                let Some(offer) = id else { return };
                let files = state
                    .mimes
                    .get(&offer.id())
                    .is_some_and(|m| m.iter().any(|m| m == URI_LIST));
                if !files {
                    offer.accept(serial, None);
                    state.forget(&offer);
                    return;
                }
                offer.accept(serial, Some(URI_LIST.into()));
                if offer.version() >= 3 {
                    use wl_data_device_manager::DndAction;
                    offer.set_actions(DndAction::Copy, DndAction::Copy);
                }
                state.current = Some(offer);
                state.set_hover(Some((x, y)));
            }
            Event::Motion { x, y, .. } => {
                if state.current.is_some() {
                    state.set_hover(Some((x, y)));
                }
            }
            Event::Leave => {
                if let Some(old) = state.current.take() {
                    state.forget(&old);
                }
                state.set_hover(None);
            }
            Event::Drop => {
                if let Some(offer) = state.current.take() {
                    state.receive(offer);
                }
            }
            // Clipboard offers belong to egui's clipboard; let go.
            Event::Selection { id: Some(offer) } => state.forget(&offer),
            _ => {}
        }
    }

    event_created_child!(State, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ()),
    ]);
}

/// Waits up to `ms` for `fd` to become readable.
fn wait_readable(fd: std::os::fd::BorrowedFd, ms: i32) -> std::io::Result<bool> {
    use std::os::fd::AsRawFd;
    let mut pfd = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd on the stack.
    let n = unsafe { libc::poll(&mut pfd, 1, ms) };
    if n < 0 {
        let err = std::io::Error::last_os_error();
        return match err.kind() {
            std::io::ErrorKind::Interrupted => Ok(false),
            _ => Err(err),
        };
    }
    Ok(n > 0)
}

/// Local paths in a `text/uri-list` body (RFC 2483): one URI per line,
/// `#` comments, `file://` URIs with percent-encoding.
fn parse_uri_list(data: &[u8]) -> Vec<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    String::from_utf8_lossy(data)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|uri| uri.strip_prefix("file://"))
        // `file://host/path`: skip the host part.
        .filter_map(|rest| rest.find('/').map(|i| &rest[i..]))
        .map(|path| PathBuf::from(std::ffi::OsString::from_vec(percent_decode(path))))
        .collect()
}

fn percent_decode(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_lists_decode_to_local_paths() {
        let body = b"# from a file manager\r\nfile:///home/me/My%20Clips/a%2Bb.mp4\r\nfile://host/tmp/x.mkv\r\nhttps://example.com/v.mp4\r\nfile:///tmp/100%\r\n";
        assert_eq!(
            parse_uri_list(body),
            vec![
                PathBuf::from("/home/me/My Clips/a+b.mp4"),
                PathBuf::from("/tmp/x.mkv"),
                PathBuf::from("/tmp/100%"),
            ]
        );
    }
}
