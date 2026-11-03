use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::json;

use super::net::Pipe;
use super::{CursorImage, Damage};

/// SCTP's default per-message cap (65536 - 1).
const MAX_MESSAGE: usize = u16::MAX as usize;

/// A real-time sink backed by one open pipe.
pub struct PipeSink {
    pipe: Pipe,
}

impl PipeSink {
    /// Hold the pipe the host bridged to the channel.
    pub fn new(pipe: Pipe) -> Self {
        Self { pipe }
    }

    /// Send the stream's shape.
    pub async fn announce(
        &mut self,
        width: u16,
        height: u16,
        source: (u16, u16),
    ) -> Result<(), String> {
        let (sw, sh) = source;
        let msg = format!(
            r#"{{"screen":{{"width":{width},"height":{height},"source":{{"width":{sw},"height":{sh}}}}}}}"#
        );
        self.pipe.text(&msg).await.map_err(|e| e.to_string())
    }

    /// Send raw image and its metadata of the cursor.
    pub async fn cursor(&mut self, image: &CursorImage) -> Result<(), String> {
        let msg = json!({
            "cursor": {
                "width": image.width,
                "height": image.height,
                "xhot": image.xhot,
                "yhot": image.yhot,
                "data": B64.encode(&image.pixels),
            }
        });
        self.pipe
            .text(&msg.to_string())
            .await
            .map_err(|e| e.to_string())
    }

    /// Process and send a frame.
    pub async fn push(&mut self, frame: &[(Damage, &[u8])]) -> Result<(), String> {
        let items: Vec<(Damage, &[u8])> = frame
            .iter()
            .filter(|(r, payload)| r.width > 0 && r.height > 0 && !payload.is_empty())
            .map(|(r, payload)| (*r, *payload))
            .collect();
        if items.is_empty() {
            return Ok(());
        }

        let total: usize = items.iter().map(|(_, payload)| payload.len()).sum();
        let mut header = String::from("{\"frame\":[");
        for (i, (r, payload)) in items.iter().enumerate() {
            if i > 0 {
                header.push(',');
            }
            header.push_str(&format!(
                "{{\"x\":{},\"y\":{},\"w\":{},\"h\":{},\"len\":{}}}",
                r.x,
                r.y,
                r.width,
                r.height,
                payload.len()
            ));
        }
        header.push_str(&format!("],\"total\":{total}}}"));
        self.pipe.text(&header).await.map_err(|e| e.to_string())?;

        for (_, payload) in &items {
            for chunk in payload.chunks(MAX_MESSAGE) {
                self.pipe.binary(chunk).await.map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
}
