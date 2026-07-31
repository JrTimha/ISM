//! Pure helpers with no dependencies of their own.
//!
//! Anything here must be a function of its arguments. `check_user_in_room` used to live in this
//! module and reached into `AppState`'s room repository — which let handlers query the database
//! through the back door. Room membership is now checked by the services that own the room, where
//! it can be enforced for every caller rather than for the handlers that remembered to ask.

use bytes::Bytes;
use image::{GenericImageView, ImageError};
use serde::Serializer;
use std::io::Cursor;

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

pub fn truncate_and_serialize<S>(text: &String, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if text.chars().count() > 50 {
        let mut truncated = text.chars().take(40).collect::<String>();
        truncated.push_str("...");
        serializer.serialize_str(&truncated)
    } else {
        serializer.serialize_str(text)
    }
}
