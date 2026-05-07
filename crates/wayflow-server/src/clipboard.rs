use tokio::sync::mpsc;
use wayflow_proto::ClipboardContent;

#[cfg(target_os = "linux")]
mod platform {
    use std::{borrow::Cow, thread, time::Duration};

    use arboard::{Clipboard, ImageData};
    use tokio::sync::mpsc;
    use tracing::{debug, warn};
    use wayflow_proto::{ClipboardContent, ClipboardImage};

    const POLL_INTERVAL: Duration = Duration::from_millis(750);
    const MAX_CLIPBOARD_BYTES: usize = 3 * 1024 * 1024;

    pub fn start(outbound: mpsc::Sender<ClipboardContent>) -> mpsc::Sender<ClipboardContent> {
        let (apply_tx, mut apply_rx) = mpsc::channel::<ClipboardContent>(16);
        thread::spawn(move || {
            let mut clipboard = match Clipboard::new() {
                Ok(clipboard) => clipboard,
                Err(e) => {
                    warn!("clipboard unavailable: {e}");
                    return;
                }
            };
            let mut last_seen: Option<ClipboardContent> = None;

            loop {
                while let Ok(content) = apply_rx.try_recv() {
                    if let Err(e) = set_clipboard(&mut clipboard, &content) {
                        warn!("clipboard apply failed: {e}");
                    } else {
                        last_seen = Some(content);
                    }
                }

                if let Some(content) = snapshot(&mut clipboard) {
                    if content_size(&content) > MAX_CLIPBOARD_BYTES {
                        warn!("clipboard content too large; skipping sync");
                    } else if last_seen.as_ref() != Some(&content) {
                        debug!("local clipboard changed: {}", content_label(&content));
                        last_seen = Some(content.clone());
                        if outbound.blocking_send(content).is_err() {
                            break;
                        }
                    }
                }

                thread::sleep(POLL_INTERVAL);
            }
        });
        apply_tx
    }

    fn snapshot(clipboard: &mut Clipboard) -> Option<ClipboardContent> {
        if let Ok(text) = clipboard.get_text() {
            return Some(ClipboardContent::Text(text));
        }

        let image = clipboard.get_image().ok()?;
        Some(ClipboardContent::Image(ClipboardImage {
            width: image.width as u32,
            height: image.height as u32,
            rgba: image.bytes.into_owned(),
        }))
    }

    fn set_clipboard(clipboard: &mut Clipboard, content: &ClipboardContent) -> anyhow::Result<()> {
        match content {
            ClipboardContent::Text(text) => {
                clipboard.set_text(text.clone())?;
            }
            ClipboardContent::Image(image) => {
                clipboard.set_image(ImageData {
                    width: image.width as usize,
                    height: image.height as usize,
                    bytes: Cow::Owned(image.rgba.clone()),
                })?;
            }
        }
        Ok(())
    }

    fn content_size(content: &ClipboardContent) -> usize {
        match content {
            ClipboardContent::Text(text) => text.len(),
            ClipboardContent::Image(image) => image.rgba.len(),
        }
    }

    fn content_label(content: &ClipboardContent) -> &'static str {
        match content {
            ClipboardContent::Text(_) => "text",
            ClipboardContent::Image(_) => "image",
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use tokio::sync::mpsc;
    use wayflow_proto::ClipboardContent;

    pub fn start(_outbound: mpsc::Sender<ClipboardContent>) -> mpsc::Sender<ClipboardContent> {
        let (apply_tx, _apply_rx) = mpsc::channel::<ClipboardContent>(1);
        apply_tx
    }
}

pub fn start(outbound: mpsc::Sender<ClipboardContent>) -> mpsc::Sender<ClipboardContent> {
    platform::start(outbound)
}
