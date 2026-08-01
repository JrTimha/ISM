//! Pure helpers with no dependencies of their own.
//!
//! Anything here must be a function of its arguments. `check_user_in_room` used to live in this
//! module and reached into `AppState`'s room repository — which let handlers query the database
//! through the back door. Room membership is now checked by the services that own the room, where
//! it can be enforced for every caller rather than for the handlers that remembered to ask.

use bytes::Bytes;
use image::{GenericImageView, ImageError};
use std::io::Cursor;

/// Length above which a room preview is shortened, and the length it is shortened to.
///
/// The gap matters: a 51-character text becomes 43 characters, so re-truncating an already
/// truncated value is a no-op. That is what makes it safe to move truncation off the write path
/// and onto the read path without rewriting history.
const PREVIEW_TRUNCATE_ABOVE: usize = 50;
const PREVIEW_TRUNCATE_TO: usize = 40;

pub fn crop_image_from_center(data: &Bytes, target_width: u32, target_height: u32) -> Result<Bytes, ImageError> {
    let img = match image::load_from_memory(data) {
        Ok(img) => img,
        Err(err) => return Err(err),
    };

    let (original_width, original_height) = img.dimensions();

    if original_width < target_width || original_height < target_height {
        return Ok(data.clone());
    };

    let x = (original_width - target_width) / 2;
    let y = (original_height - target_height) / 2;
    let cropped = img.crop_imm(x, y, target_width, target_height).to_rgb8();

    let mut buffer = Cursor::new(Vec::new());
    match cropped.write_to(&mut buffer, image::ImageFormat::Jpeg) {
        Ok(_) => Ok(Bytes::from(buffer.into_inner())),
        Err(err) => Err(err),
    }
}

/// Shortens a room-list preview for display.
///
/// This used to be a `#[serde(serialize_with = ...)]` on `LastMessagePreviewText`, which was both
/// the JSONB column type and the response type — so the truncation also ran on the `INSERT` path
/// and the *stored* preview was already shortened. A display concern cannot be expressed as a
/// `Serialize` impl on a type that is also a storage format; it belongs in the conversion from the
/// stored value to the response, which is where this is now called from.
pub fn truncate_preview(text: &str) -> String {
    if text.chars().count() > PREVIEW_TRUNCATE_ABOVE {
        let mut truncated = text.chars().take(PREVIEW_TRUNCATE_TO).collect::<String>();
        truncated.push_str("...");
        truncated
    } else {
        text.to_string()
    }
}
