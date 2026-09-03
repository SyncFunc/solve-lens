use crate::domain::CapturedImage;
use anyhow::{bail, Context, Result};
use chrono::Utc;
use screenshots::Screen;
use std::fs;
use std::path::Path;
use uuid::Uuid;

/// Captures the primary monitor. `screenshots` uses the current platform's native
/// capture path; this module is intentionally the single replaceable CaptureSource.
pub fn capture_primary(work_dir: &Path) -> Result<CapturedImage> {
    let capture_dir = work_dir.join("captures");
    fs::create_dir_all(&capture_dir).context("create capture directory")?;

    let screen = Screen::all()
        .context("enumerate displays")?
        .into_iter()
        .find(|item| item.display_info.is_primary)
        .ok_or_else(|| anyhow::anyhow!("no primary display found"))?;
    let image = screen.capture().context("capture primary display")?;
    let path = capture_dir.join(format!("{}.png", Uuid::new_v4()));
    image.save(&path).context("save screenshot")?;

    if !path.exists() {
        bail!("screen capture did not create an image");
    }
    Ok(CapturedImage {
        path,
        captured_at: Utc::now(),
    })
}
