use std::sync::Arc;
use bytes::Bytes;
use uuid::Uuid;
use std::io::Cursor;
use image::{GenericImageView, ImageError};
use crate::errors::{AppError};
use crate::core::AppState;


pub async fn check_user_in_room(
    state: &Arc<AppState>,
    user_id: &Uuid,
    room_id: &Uuid,
) -> Result<(), AppError> {
    let is_in = state.room_repository.is_user_in_room(user_id, room_id).await?;
    if is_in {
        Ok(())
    } else {
        Err(AppError::Blocked("Invalid permissions to interact with this room".to_string()))
    }
}

pub fn crop_image_from_center(
    data: &Bytes,
    target_width: u32,
    target_height: u32,
) -> Result<Bytes, ImageError> {

    let img = match image::load_from_memory(data) {
        Ok(img) => img,
        Err(err) => return Err(err)
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
        Err(err) => Err(err)
    }
}


