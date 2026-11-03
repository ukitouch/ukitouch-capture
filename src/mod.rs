//! TODO: Currently only x11 implemented.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Interval to ask the backend for changes.
///
/// This is a poll interval, not a frame rate.
const POLL_INTERVAL: Duration = Duration::from_millis(16);

/// A changed rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Damage {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

/// A screen's dimensions.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ScreenInfo {
    pub width: u16,
    pub height: u16,
}

/// A started capture: the id and the screen's shape.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Started {
    pub id: u64,
    pub width: u16,
    pub height: u16,
    /// The loopback address the host's pipe connects to.
    pub addr: String,
}

/// Capture errors.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CaptureError {
    Denied,
    Cancelled,
    /// this platform has no backend
    Unsupported {
        reason: String,
    },
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Denied => write!(f, "screen capture was denied"),
            Self::Cancelled => write!(f, "screen capture was cancelled"),
            Self::Unsupported { reason } => write!(f, "screen capture unsupported: {reason}"),
        }
    }
}

impl std::error::Error for CaptureError {}

/// The cursor's displaying way.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Cursor {
    #[default]
    Off,
    /// Drawn into the frames' pixels.
    Composite,
    /// Sent the cursor's raw image to the peers to draw by themselves.
    Raw,
}

/// Capture options to the screen collector.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CaptureOptions {
    pub cursor: Cursor,
}

/// The cursor's own image.
pub struct CursorImage {
    pub width: u16,
    pub height: u16,
    /// The cursor's own position within the image.
    pub xhot: u16,
    pub yhot: u16,
    /// Row-major premultiplied RGBA.
    pub pixels: Vec<u8>,
}

/// Screen collector's backend.
pub trait CaptureBackend: Send {
    /// Screen dimensions and pixel layout.
    fn info(&self) -> ScreenInfo;

    /// Grab the screen and report what changed since the previous call.
    ///
    /// Returns `None` when nothing changed.
    ///
    /// Implementations should consult the platform's own "something was
    /// redrawn" signal before doing any pixel comparison.
    fn poll(&mut self) -> Result<Option<Vec<(Damage, Vec<u8>)>>, CaptureError>;

    /// Force the next `poll` to report the whole screen again.
    fn resync(&mut self);

    /// Apply options. Returning `true` when the screen's shape changed.
    fn configure(&mut self, _options: &CaptureOptions) -> bool {
        false
    }

    /// The pointer's own image.
    ///
    /// Returns `Some` only when the image changed since the last call.
    fn cursor_image(&mut self) -> Result<Option<CursorImage>, CaptureError> {
        Ok(None)
    }
}

pub mod net;
pub mod sink;
pub mod stream;

#[cfg(all(unix, not(target_os = "android")))]
pub mod x11;

/// Build the capturing backend for this platform.
pub fn create() -> Result<Box<dyn CaptureBackend>, CaptureError> {
    #[cfg(all(unix, not(target_os = "android")))]
    {
        return x11::X11Capture::new().map(|b| Box::new(b) as Box<dyn CaptureBackend>);
    }
    #[cfg(not(all(unix, not(target_os = "android"))))]
    {
        Err(CaptureError::Unsupported {
            reason: format!("no capture backend for {}", std::env::consts::OS),
        })
    }
}
