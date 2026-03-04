use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Serialize, Clone)]
pub struct FirmwareInfo {
    version: String,
    download_url: String,
    release_tag: String,
}

#[tauri::command]
pub async fn fetch_latest_firmware() -> Result<FirmwareInfo, String> {
    let client = reqwest::Client::builder()
        .user_agent("pointify")
        .build()
        .map_err(|e| e.to_string())?;

    let resp: serde_json::Value = client
        .get("https://api.github.com/repos/luftaquila/pointify/releases/latest")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let release_tag = resp["tag_name"]
        .as_str()
        .unwrap_or("")
        .to_string();

    let assets = resp["assets"].as_array().ok_or("No assets in release")?;

    for asset in assets {
        let name = asset["name"].as_str().unwrap_or("");
        if name.starts_with("pointify-firmware-") && name.ends_with(".elf") {
            let version = name
                .strip_prefix("pointify-firmware-")
                .and_then(|s| s.strip_suffix(".elf"))
                .unwrap_or("unknown")
                .to_string();
            let download_url = asset["browser_download_url"]
                .as_str()
                .ok_or("No download URL")?
                .to_string();
            return Ok(FirmwareInfo {
                version,
                download_url,
                release_tag,
            });
        }
    }

    Err("No firmware asset found".to_string())
}

#[tauri::command]
pub async fn download_firmware(
    app: AppHandle,
    download_url: String,
) -> Result<(), String> {
    let _ = app.emit("flash-output", "Downloading firmware...");

    let client = reqwest::Client::builder()
        .user_agent("pointify")
        .build()
        .map_err(|e| e.to_string())?;

    let elf_bytes = client
        .get(&download_url)
        .send()
        .await
        .map_err(|e| format!("Download failed: {}", e))?
        .error_for_status()
        .map_err(|e| format!("Download failed: {}", e))?
        .bytes()
        .await
        .map_err(|e| format!("Download failed: {}", e))?
        .to_vec();

    if elf_bytes.is_empty() {
        return Err("Download failed: empty response".to_string());
    }

    let _ = app.emit("flash-output", &format!("Downloaded {} bytes", elf_bytes.len()));

    let cache_dir = app.path().app_cache_dir().map_err(|e| e.to_string())?;
    let _ = std::fs::create_dir_all(&cache_dir);
    let elf_path = cache_dir.join("firmware.elf");
    std::fs::write(&elf_path, &elf_bytes).map_err(|e| format!("Save failed: {}", e))?;

    Ok(())
}

#[tauri::command]
pub async fn flash_firmware(app: AppHandle) -> Result<(), String> {
    let cache_dir = app.path().app_cache_dir().map_err(|e| e.to_string())?;
    let elf_path = cache_dir.join("firmware.elf");

    if !elf_path.exists() {
        return Err("No firmware file found. Download firmware first.".to_string());
    }

    let elf_path_str = elf_path.to_str().ok_or("Invalid path")?.to_string();

    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let emit = |msg: &str| {
            let _ = app.emit("flash-output", msg);
        };

        // Parse ELF to raw binary
        emit("Parsing firmware...");
        let raw = wchisp::format::read_firmware_from_file(&elf_path_str)
            .map_err(|e| format!("Parse failed: {}", e))?;

        // Pad to sector boundary (1024 bytes)
        let mut binary = raw;
        if binary.len() % 1024 != 0 {
            binary.resize(binary.len() + (1024 - binary.len() % 1024), 0);
        }
        emit(&format!("Firmware: {} bytes", binary.len()));

        // Connect to bootloader
        emit("Connecting to bootloader...");
        let mut flashing = wchisp::Flashing::new_from_usb(None)
            .map_err(|e| format!("Connect failed: {}", e))?;
        emit(&format!("Connected: {}", flashing.chip.name));

        // Erase
        let sectors = (binary.len() / 1024) + 1;
        emit(&format!("Erasing {} sectors...", sectors));
        flashing
            .erase_code(sectors as u32)
            .map_err(|e| format!("Erase failed: {}", e))?;

        std::thread::sleep(std::time::Duration::from_secs(1));

        // Flash
        emit("Flashing...");
        flashing
            .flash(&binary)
            .map_err(|e| format!("Flash failed: {}", e))?;

        std::thread::sleep(std::time::Duration::from_millis(500));

        // Verify
        emit("Verifying...");
        flashing
            .verify(&binary)
            .map_err(|e| format!("Verify failed: {}", e))?;

        // Reset
        emit("Resetting device...");
        flashing
            .reset()
            .map_err(|e| format!("Reset failed: {}", e))?;

        emit("Firmware update complete!");
        Ok(())
    })
    .await
    .map_err(|e| format!("Task failed: {}", e))?
}
