use std::sync::Arc;
use bytes::Bytes;
use uuid::Uuid;
use std::io::Cursor;
use http::StatusCode;
use image::GenericImageView;
use log::error;
use crate::errors::{ErrorCode, HttpError};
use crate::core::AppState;


pub async fn check_user_in_room(
    state: &Arc<AppState>,
    user_id: &Uuid,
    room_id: &Uuid,
) -> Result<(), HttpError> {
    let is_in = match state
        .room_repository
        .is_user_in_room(user_id, room_id)
        .await {
        Ok(is_in) => is_in,
        Err(err) => {
            error!("{}", err);
            return Err(HttpError::new(StatusCode::INTERNAL_SERVER_ERROR, ErrorCode::UnexpectedError, "Unable to check if user is in room"))
        }
    };

    if is_in {
        Ok(())
    } else {
        Err(HttpError::new(StatusCode::UNAUTHORIZED, ErrorCode::InsufficientPermissions, "Unable to check if user is in room"))
    }
}

pub fn crop_image_from_center(
    data: &Bytes,
    target_width: u32,
    target_height: u32,
) -> Result<Bytes, HttpError> {

    let img = match image::load_from_memory(data) {
        Ok(img) => img,
        Err(err) => {
            error!("{}", err);
            return Err(HttpError::new(StatusCode::BAD_REQUEST, ErrorCode::FileProcessingError, "Unable to load the image."))
        }
    };

    let (original_width, original_height) = img.dimensions();

    if original_width < target_width || original_height < target_height {
        return Ok(data.clone())
    };

    let x = (original_width - target_width) / 2;
    let y = (original_height - target_height) / 2;
    let cropped = img.crop_imm(x, y, target_width, target_height).to_rgb8();

    let mut buffer = Cursor::new(Vec::new());
    match cropped.write_to(&mut buffer, image::ImageFormat::Jpeg){
        Ok(_) => {
            Ok(Bytes::from(buffer.into_inner()))
        },
        Err(err) => {
            error!("{}", err);
            Err(HttpError::new(StatusCode::BAD_REQUEST, ErrorCode::FileProcessingError, "Image processing failed."))
        }
    }
}
