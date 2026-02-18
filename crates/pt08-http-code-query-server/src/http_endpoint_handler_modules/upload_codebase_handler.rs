//! Endpoint: POST /upload-codebase-zip
//!
//! Accepts an application/octet-stream (zip) in the request body and saves it
//! under `/data/uploads/` with a timestamped filename. Returns the saved path.
//! Note: ingestion (pt01) can be run separately against the extracted folder.
use axum::{
    extract::{State, Body},
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
    body: Body,
) -> impl IntoResponse {
    // update last request timestamp
    state.update_last_request_timestamp().await;

    // read body bytes
    let bytes = match hyper::body::to_bytes(body).await {
        Ok(b) => b,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, Json(UploadResponse {
                success: false,
                message: format!("Failed to read body: {}", e),
                saved_path: "".to_string(),
            }));
        }
    };

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

    // Run ingestion (pt01) with working dir = /data so the timestampped workspace lands in /data
    let ingest_cmd = Command::new("/usr/local/bin/parseltongue")
        .arg("pt01-folder-to-cozodb-streamer")
        .arg(extract_dir.to_string_lossy().to_string())
        .current_dir("/data")
        .output()
        .await;

    if let Err(e) = ingest_cmd {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(UploadResponse {
            success: false,
            message: format!("Failed to run ingestion command: {}", e),
            saved_path: saved_path.to_string_lossy().to_string(),
        }));
    }

    let out = ingest_cmd.unwrap();
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(UploadResponse {
            success: false,
            message: format!("Ingestion failed: {}", stderr),
            saved_path: saved_path.to_string_lossy().to_string(),
        }));
    }

    // Find the most recent parseltongue* directory under /data (the created workspace)
    let mut newest: Option<(PathBuf, SystemTime)> = None;
    match fs::read_dir("/data").await {
        Ok(mut rd) => {
            while let Ok(Some(entry)) = rd.next_entry().await {
                let file_name = entry.file_name();
                let fname = file_name.to_string_lossy().to_string();
                if fname.starts_with("parseltongue") {
                    if let Ok(meta) = entry.metadata().await {
                        if let Ok(mtime) = meta.modified() {
                            match &newest {
                                Some((_, t)) if *t >= mtime => {},
                                _ => newest = Some((entry.path(), mtime)),
                            }
                        }
                    }
                }
            }
        }
        Err(_) => {}
    }

    let mut db_path = "".to_string();
    if let Some((pathbuf, _)) = newest {
        // create/update /data/current symlink to point to this workspace
        let current_link = PathBuf::from("/data/current");
        let _ = fs::remove_file(&current_link).await; // ignore error if not exist
        // create symlink (std::os::unix)
        if let Err(e) = unix_fs::symlink(&pathbuf, &current_link) {
            // non-fatal; include in response message
            db_path = pathbuf.to_string_lossy().to_string();
            return (StatusCode::OK, Json(UploadResponse {
                success: true,
                message: format!("Uploaded and ingested OK, but failed to create /data/current symlink: {}", e),
                saved_path: saved_path.to_string_lossy().to_string(),
            }));
        } else {
            db_path = current_link.join("analysis.db").to_string_lossy().to_string();
        }
    } else {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(UploadResponse {
            success: false,
            message: "Ingestion completed but workspace not found under /data".to_string(),
            saved_path: saved_path.to_string_lossy().to_string(),
        }));
    }

    let saved_path_str = saved_path.to_string_lossy().to_string();
    (
        StatusCode::OK,
        Json(UploadResponse {
            success: true,
            message: format!("Uploaded zip saved and ingested successfully. DB path: {}", db_path),
            saved_path: saved_path_str,
        }),
    )
}

