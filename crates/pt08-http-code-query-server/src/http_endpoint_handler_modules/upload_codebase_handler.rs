//! Endpoint: POST /upload-codebase-zip
//!
//! Accepts an application/octet-stream (zip) in the request body and saves it
//! under `/data/uploads/` with a timestamped filename. Returns the saved path.
//! Note: ingestion (pt01) can be run separately against the extracted folder.
use axum::{
    extract::State,
    body::Bytes,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::Utc;
use serde::Serialize;
use std::path::PathBuf;
use tokio::fs;
use tokio::process::Command;
use std::time::SystemTime;
use std::os::unix::fs as unix_fs;
use crate::http_server_startup_runner::SharedApplicationStateContainer;

#[derive(Debug, Serialize)]
struct UploadResponse {
    success: bool,
    message: String,
    saved_path: String,
}

pub async fn handle_upload_codebase_zip(
    State(state): State<SharedApplicationStateContainer>,
    body: Bytes,
) -> impl IntoResponse {
    // update last request timestamp
    state.update_last_request_timestamp().await;

    // body is already collected bytes
    let bytes = body;

    // ensure uploads dir
    let uploads_dir = PathBuf::from("/data/uploads");
    if let Err(e) = fs::create_dir_all(&uploads_dir).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(UploadResponse {
            success: false,
            message: format!("Failed to create uploads dir: {}", e),
            saved_path: "".to_string(),
        }));
    }

    // write file with timestamp
    let ts = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let filename = format!("upload-{}.zip", ts);
    let saved_path = uploads_dir.join(&filename);
    if let Err(e) = fs::write(&saved_path, &bytes).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(UploadResponse {
            success: false,
            message: format!("Failed to write file: {}", e),
            saved_path: "".to_string(),
        }));
    }

    // try to extract the zip into a timestamped folder under /data/uploads/extracted-<ts>
    let extract_dir = uploads_dir.join(format!("extracted-{}", ts));
    if let Err(e) = fs::create_dir_all(&extract_dir).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(UploadResponse {
            success: false,
            message: format!("Failed to create extract dir: {}", e),
            saved_path: saved_path.to_string_lossy().to_string(),
        }));
    }

    // unzip using system 'unzip' which is available in the runtime image
    let unzip_status = Command::new("unzip")
        .arg("-q")
        .arg(&saved_path)
        .arg("-d")
        .arg(&extract_dir)
        .status()
        .await;

    if let Err(e) = unzip_status {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(UploadResponse {
            success: false,
            message: format!("Failed to run unzip: {}", e),
            saved_path: saved_path.to_string_lossy().to_string(),
        }));
    }

    let status = unzip_status.unwrap();
    if !status.success() {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(UploadResponse {
            success: false,
            message: format!("unzip failed with exit code: {}", status.code().unwrap_or(-1)),
            saved_path: saved_path.to_string_lossy().to_string(),
        }));
    }

    // Do not run ingestion inside the server (not feasible on some platforms).
    // The handler only saves and extracts the uploaded archive. Ingestion should be
    // run as a separate one-off process that writes a workspace under /data.
    let saved_path_str = saved_path.to_string_lossy().to_string();
    let extract_path_str = extract_dir.to_string_lossy().to_string();
    (
        StatusCode::OK,
        Json(UploadResponse {
            success: true,
            message: format!("Uploaded and extracted successfully. Extracted path: {}", extract_path_str),
            saved_path: saved_path_str,
        }),
    )
}

