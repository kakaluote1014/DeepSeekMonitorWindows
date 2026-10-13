mod material;

// --- DPAPI (Windows Data Protection API)：凭据落盘前加密 ---
#[repr(C)]
struct DataBlob {
    cb_data: u32,
    pb_data: *mut u8,
}

#[link(name = "crypt32")]
extern "system" {
    fn CryptProtectData(
        pdata_in: *const DataBlob,
        sz_data_descr: *const u16,
        p_optional_entropy: *const DataBlob,
        pv_reserved: *mut core::ffi::c_void,
        p_prompt_struct: *const core::ffi::c_void,
        dw_flags: u32,
        pdata_out: *mut DataBlob,
    ) -> i32;

    fn CryptUnprotectData(
        pdata_in: *const DataBlob,
        p_sz_data_descr: *mut *mut u16,
        p_optional_entropy: *const DataBlob,
        pv_reserved: *mut core::ffi::c_void,
        p_prompt_struct: *const core::ffi::c_void,
        dw_flags: u32,
        pdata_out: *mut DataBlob,
    ) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn LocalFree(h_mem: isize) -> isize;
}

fn dpapi_encrypt(plain: &[u8]) -> Result<Vec<u8>, String> {
    let data_in = DataBlob {
        cb_data: plain.len() as u32,
        pb_data: plain.as_ptr() as *mut u8,
    };
    let mut data_out = DataBlob {
        cb_data: 0,
        pb_data: std::ptr::null_mut(),
    };
    let result = unsafe {
        CryptProtectData(
            &data_in,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null(),
            0,
            &mut data_out,
        )
    };
    if result == 0 {
        return Err("DPAPI 加密失败".to_string());
    }
    let encrypted = unsafe {
        std::slice::from_raw_parts(data_out.pb_data, data_out.cb_data as usize).to_vec()
    };
    unsafe {
        LocalFree(data_out.pb_data as isize);
    }
    Ok(encrypted)
}

fn dpapi_decrypt(encrypted: &[u8]) -> Result<Vec<u8>, String> {
    let data_in = DataBlob {
        cb_data: encrypted.len() as u32,
        pb_data: encrypted.as_ptr() as *mut u8,
    };
    let mut data_out = DataBlob {
        cb_data: 0,
        pb_data: std::ptr::null_mut(),
    };
    let result = unsafe {
        CryptUnprotectData(
            &data_in,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null(),
            0,
            &mut data_out,
        )
    };
    if result == 0 {
        return Err("DPAPI 解密失败，凭据可能由其他 Windows 用户或系统加密".to_string());
    }
    let decrypted = unsafe {
        std::slice::from_raw_parts(data_out.pb_data, data_out.cb_data as usize).to_vec()
    };
    unsafe {
        LocalFree(data_out.pb_data as isize);
    }
    Ok(decrypted)
}

fn hex_encode(data: &[u8]) -> String {
    data.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hex_decode(hex: &str) -> Result<Vec<u8>, String> {
    if hex.len() % 2 != 0 {
        return Err("十六进制编码长度无效".to_string());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| format!("十六进制解码失败：{e}")))
        .collect()
}

fn encrypt_credential(plain: &str) -> String {
    match dpapi_encrypt(plain.as_bytes()) {
        Ok(encrypted) => format!("enc1:{}", hex_encode(&encrypted)),
        Err(_) => {
            log::warn!("DPAPI 加密失败，将明文保存凭据");
            plain.to_string()
        }
    }
}

fn decrypt_credential(stored: &str) -> Result<String, String> {
    if let Some(hex) = stored.strip_prefix("enc1:") {
        let encrypted = hex_decode(hex)?;
        let decrypted = dpapi_decrypt(&encrypted)?;
        String::from_utf8(decrypted).map_err(|e| format!("解密凭据失败：{e}"))
    } else {
        Ok(stored.to_string())
    }
}

/// 判断磁盘上的凭据是否已经是 DPAPI 密文（旧版本写入的是明文）。
fn is_encrypted_credential(stored: &str) -> bool {
    stored.starts_with("enc1:")
}

/// 模型 ID → 仪表盘分组（分组 key，展示名）。
///
/// DeepSeek 会重命名、下线或合并模型 ID：`deepseek-flash`（V4.1 Flash）已经取代
/// `deepseek-v4-flash`，`deepseek-v4.1-flash-expires-on-0910` 是过渡别名，
/// 而 `deepseek-v4-flash-vision-exp` 是独立的视觉模型。平台在同一个响应里会同时
/// 返回新旧 ID，所以这里按前缀归并，避免改名后用量被静默丢弃。
fn model_group(model: &str) -> Option<(&'static str, &'static str)> {
    let name = model.trim().to_ascii_lowercase();

    // 视觉模型单独成行，必须先于 flash 前缀判断（它同样以 deepseek-v4-flash 开头）。
    if name.contains("vision") {
        return Some(("flashvision", "V4 Flash Vision"));
    }
    if name.starts_with("deepseek-flash")
        || name.starts_with("deepseek-v4-flash")
        || name.starts_with("deepseek-v4.1-flash")
    {
        return Some(("flash", "V4 Flash"));
    }
    if name.starts_with("deepseek-v4-pro") || name.starts_with("deepseek-pro") {
        return Some(("pro", "V4 Pro"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let data = vec![0u8, 1, 15, 16, 127, 128, 255];
        let encoded = hex_encode(&data);
        assert_eq!(encoded, "00010f107f80ff");
        assert_eq!(hex_decode(&encoded).unwrap(), data);
    }

    #[test]
    fn hex_decode_rejects_invalid_input() {
        assert!(hex_decode("abc").is_err());
        assert!(hex_decode("zz").is_err());
    }

    #[test]
    fn credential_roundtrip_is_encrypted() {
        let stored = encrypt_credential("sk-test-1234567890");
        assert!(is_encrypted_credential(&stored), "凭据应以 enc1: 开头");
        assert!(!stored.contains("sk-test-1234567890"), "密文中不应出现明文");
        assert_eq!(decrypt_credential(&stored).unwrap(), "sk-test-1234567890");
    }

    #[test]
    fn plaintext_credential_is_passed_through_for_migration() {
        let stored = "sk-legacy-plaintext";
        assert!(!is_encrypted_credential(stored));
        assert_eq!(decrypt_credential(stored).unwrap(), stored);
    }

    #[test]
    fn decrypt_rejects_corrupted_ciphertext() {
        assert!(decrypt_credential("enc1:00ff00ff").is_err());
        assert!(decrypt_credential("enc1:not-hex").is_err());
    }

    #[test]
    fn model_group_matches_current_platform_ids() {
        // 2026-10 平台用量接口实际返回的模型 ID。
        assert_eq!(model_group("deepseek-flash"), Some(("flash", "V4 Flash")));
        assert_eq!(model_group("deepseek-v4-pro"), Some(("pro", "V4 Pro")));
        assert_eq!(
            model_group("deepseek-v4-flash-vision-exp"),
            Some(("flashvision", "V4 Flash Vision"))
        );
    }

    #[test]
    fn model_group_maps_legacy_and_alias_ids() {
        assert_eq!(model_group("deepseek-v4-flash"), Some(("flash", "V4 Flash")));
        assert_eq!(
            model_group("deepseek-v4.1-flash-expires-on-0910"),
            Some(("flash", "V4 Flash"))
        );
        assert_eq!(model_group("  DEEPSEEK-FLASH  "), Some(("flash", "V4 Flash")));
        assert_eq!(model_group("deepseek-pro"), Some(("pro", "V4 Pro")));
    }

    #[test]
    fn model_group_ignores_unrelated_ids() {
        assert_eq!(model_group("deepseek-chat & deepseek-reasoner"), None);
        assert_eq!(model_group("deepseek-reasoner"), None);
        assert_eq!(model_group(""), None);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use serde::{Deserialize, Serialize};
    use std::{
        fs,
        io::Read,
        os::windows::fs::OpenOptionsExt,
        path::{Path, PathBuf},
        process::Command,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        thread,
        time::Duration,
    };
    use tauri::{
        menu::{Menu, MenuItem},
        tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
        webview::PageLoadEvent,
        Emitter, Manager, PhysicalPosition, Position, WebviewWindow,
    };

    #[derive(Clone, Debug, Default, Deserialize, Serialize)]
    struct StoredConfig {
        api_key: Option<String>,
        #[serde(default)]
        usage_token: Option<String>,
        refresh_interval_seconds: u64,
        #[serde(default)]
        auto_refresh_enabled: bool,
        autostart: bool,
    }

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct AppConfig {
        api_key_configured: bool,
        api_key_preview: Option<String>,
        usage_token_configured: bool,
        refresh_interval_seconds: u64,
        auto_refresh_enabled: bool,
        autostart: bool,
        config_path: String,
    }

    fn config_path() -> Result<PathBuf, String> {
        let appdata = std::env::var_os("APPDATA").ok_or("APPDATA is not available")?;
        Ok(PathBuf::from(appdata)
            .join("DeepSeekMonitorWindows")
            .join("config.json"))
    }

    fn read_stored_config() -> Result<StoredConfig, String> {
        let path = config_path()?;
        if !path.exists() {
            return Ok(StoredConfig {
                refresh_interval_seconds: 60,
                ..StoredConfig::default()
            });
        }

        let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        let mut config: StoredConfig =
            serde_json::from_str(&text).map_err(|error| error.to_string())?;

        // 解密失败时不再让整个配置读取失败：记录日志并把该凭据视为未配置，
        // 用户在设置页重新填写即可恢复。旧版本写入的明文凭据在此顺带迁移为密文。
        let mut needs_migration = false;
        if let Some(stored) = config.api_key.clone() {
            match decrypt_credential(&stored) {
                Ok(plain) => {
                    needs_migration |= !is_encrypted_credential(&stored);
                    config.api_key = Some(plain);
                }
                Err(error) => {
                    log::warn!("API Key 解密失败，已按未配置处理：{error}");
                    config.api_key = None;
                }
            }
        }
        if let Some(stored) = config.usage_token.clone() {
            match decrypt_credential(&stored) {
                Ok(plain) => {
                    needs_migration |= !is_encrypted_credential(&stored);
                    config.usage_token = Some(plain);
                }
                Err(error) => {
                    log::warn!("用量 Token 解密失败，已按未配置处理：{error}");
                    config.usage_token = None;
                }
            }
        }
        config.refresh_interval_seconds =
            normalize_refresh_interval_seconds(config.refresh_interval_seconds);

        if needs_migration {
            match write_stored_config(&config) {
                Ok(()) => log::info!("凭据已迁移为 DPAPI 加密存储"),
                Err(error) => log::warn!("明文凭据迁移为加密存储失败：{error}"),
            }
        }

        Ok(config)
    }

    fn normalize_refresh_interval_seconds(value: u64) -> u64 {
        match value {
            60 | 300 | 1800 | 3600 => value,
            _ => 60,
        }
    }

    fn write_stored_config(config: &StoredConfig) -> Result<(), String> {
        let encrypted_config = StoredConfig {
            api_key: config.api_key.as_ref().map(|k| encrypt_credential(k)),
            usage_token: config.usage_token.as_ref().map(|t| encrypt_credential(t)),
            ..config.clone()
        };
        let path = config_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }

        let text = serde_json::to_string_pretty(&encrypted_config).map_err(|error| error.to_string())?;
        fs::write(path, text).map_err(|error| error.to_string())
    }

    fn api_key_preview(api_key: &str) -> String {
        let chars: Vec<char> = api_key.chars().collect();
        if chars.len() <= 12 {
            return "已保存".to_string();
        }

        let start: String = chars.iter().take(7).collect();
        let end: String = chars
            .iter()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("{start}...{end}")
    }

    fn to_app_config(config: StoredConfig) -> Result<AppConfig, String> {
        let path = config_path()?;
        let api_key_preview = config
            .api_key
            .as_ref()
            .filter(|value| !value.is_empty())
            .map(|value| api_key_preview(value));

        let usage_token_configured = config
            .usage_token
            .as_ref()
            .map(|value| !value.is_empty())
            .unwrap_or(false);

        Ok(AppConfig {
            api_key_configured: api_key_preview.is_some(),
            api_key_preview,
            usage_token_configured,
            refresh_interval_seconds: config.refresh_interval_seconds,
            auto_refresh_enabled: config.auto_refresh_enabled,
            autostart: config.autostart,
            config_path: path.to_string_lossy().to_string(),
        })
    }

    fn position_near_tray(window: &WebviewWindow) -> tauri::Result<()> {
        let cursor = window.cursor_position()?;
        let monitor = window
            .monitor_from_point(cursor.x, cursor.y)?
            .or(window.current_monitor()?)
            .or(window.primary_monitor()?)
            .ok_or_else(|| tauri::Error::WindowNotFound)?;

        let work_area = monitor.work_area();
        let scale_factor = monitor.scale_factor();
        let size = window.outer_size()?;
        let margin = (12.0 * scale_factor).round() as i32;
        let width = size.width as i32;
        let height = size.height as i32;
        let right = work_area.position.x + work_area.size.width as i32;
        let bottom = work_area.position.y + work_area.size.height as i32;
        let x = right - width - margin;
        let y = bottom - height - margin;

        window.set_position(Position::Physical(PhysicalPosition::new(
            x.max(work_area.position.x),
            y.max(work_area.position.y),
        )))
    }

    fn show_main_window(window: &WebviewWindow) {
        let _ = position_near_tray(window);
        let _ = window.show();
        let _ = window.set_focus();
    }

    #[tauri::command]
    fn hide_main_window(window: WebviewWindow) -> Result<(), String> {
        window.hide().map_err(|error| error.to_string())
    }

    #[tauri::command]
    fn get_app_config() -> Result<AppConfig, String> {
        to_app_config(read_stored_config()?)
    }

    #[tauri::command]
    async fn set_native_material(
        window: WebviewWindow,
        enabled: bool,
        dark: bool,
    ) -> Result<crate::material::MaterialResult, String> {
        if window.label() != "main" {
            return Err("仅主窗口可以修改桌面材质".to_string());
        }
        let native_window = window.clone();
        let (sender, mut receiver) = tauri::async_runtime::channel(1);
        window.run_on_main_thread(move || {
            let result = native_window.hwnd()
                .map_err(|error| error.to_string())
                .and_then(|hwnd| crate::material::apply(hwnd.0 as isize, enabled, dark));
            let _ = sender.try_send(result);
        }).map_err(|error| error.to_string())?;
        receiver.recv().await.ok_or_else(|| "窗口效果任务未完成".to_string())?
    }

    #[tauri::command]
    fn save_api_key(api_key: String) -> Result<AppConfig, String> {
        let value = api_key.trim().to_string();
        if value.is_empty() {
            return Err("API Key 不能为空".to_string());
        }
        if value.len() > 256 {
            return Err("API Key 长度超出限制".to_string());
        }
        if !value.starts_with("sk-") {
            log::warn!("API Key 格式异常：不以 sk- 开头");
        }

        let mut config = read_stored_config()?;
        config.api_key = Some(value);
        write_stored_config(&config)?;
        to_app_config(config)
    }

    #[tauri::command]
    fn clear_api_key() -> Result<AppConfig, String> {
        let mut config = read_stored_config()?;
        config.api_key = None;
        write_stored_config(&config)?;
        to_app_config(config)
    }

    #[tauri::command]
    fn save_refresh_interval(refresh_interval_seconds: u64) -> Result<AppConfig, String> {
        let mut config = read_stored_config()?;
        config.refresh_interval_seconds =
            normalize_refresh_interval_seconds(refresh_interval_seconds);
        write_stored_config(&config)?;
        to_app_config(config)
    }

    #[tauri::command]
    fn save_auto_refresh_enabled(auto_refresh_enabled: bool) -> Result<AppConfig, String> {
        let mut config = read_stored_config()?;
        config.auto_refresh_enabled = auto_refresh_enabled;
        write_stored_config(&config)?;
        to_app_config(config)
    }

    fn apply_autostart(enabled: bool) -> Result<(), String> {
        let run_key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
        let value_name = "DeepSeekMonitorWindows";

        if enabled {
            let exe = std::env::current_exe().map_err(|error| error.to_string())?;
            let exe_arg = exe.to_string_lossy().to_string();
            let status = Command::new("reg")
                .args(["add", run_key, "/v", value_name, "/t", "REG_SZ", "/d"])
                .arg(exe_arg)
                .args(["/f"])
                .status()
                .map_err(|error| format!("写入开机自启失败：{error}"))?;
            if !status.success() {
                return Err("写入开机自启失败".to_string());
            }
            return Ok(());
        }

        let status = Command::new("reg")
            .args(["delete", run_key, "/v", value_name, "/f"])
            .status()
            .map_err(|error| format!("关闭开机自启失败：{error}"))?;
        if !status.success() {
            return Ok(());
        }
        Ok(())
    }

    #[tauri::command]
    fn save_autostart(autostart: bool) -> Result<AppConfig, String> {
        apply_autostart(autostart)?;
        let mut config = read_stored_config()?;
        config.autostart = autostart;
        write_stored_config(&config)?;
        to_app_config(config)
    }

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct BalanceResult {
        is_available: bool,
        currency: String,
        total_balance: String,
        granted_balance: String,
        topped_up_balance: String,
    }

    // 实时查询 DeepSeek 账户余额。DeepSeek 官方仅提供余额接口，无用量接口。
    #[tauri::command]
    async fn fetch_balance() -> Result<BalanceResult, String> {
        let config = read_stored_config()?;
        let api_key = config
            .api_key
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "未配置 API Key".to_string())?;

        let client = reqwest::Client::new();
        let response = client
            .get("https://api.deepseek.com/user/balance")
            .bearer_auth(&api_key)
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
            .map_err(|error| format!("网络请求失败：{error}"))?;

        match response.status().as_u16() {
            200 => {}
            401 => return Err("API Key 无效或已过期".to_string()),
            429 => return Err("请求过于频繁，请稍后再试".to_string()),
            code if code >= 500 => return Err(format!("DeepSeek 服务器错误：{code}")),
            code => return Err(format!("请求失败：HTTP {code}")),
        }

        #[derive(Deserialize)]
        struct BalanceInfo {
            currency: String,
            total_balance: String,
            granted_balance: String,
            topped_up_balance: String,
        }
        #[derive(Deserialize)]
        struct BalanceResponse {
            is_available: bool,
            balance_infos: Vec<BalanceInfo>,
        }

        let data: BalanceResponse = response
            .json()
            .await
            .map_err(|error| format!("解析余额数据失败：{error}"))?;

        let info = data
            .balance_infos
            .into_iter()
            .next()
            .ok_or_else(|| "余额信息为空".to_string())?;

        Ok(BalanceResult {
            is_available: data.is_available,
            currency: info.currency,
            total_balance: info.total_balance,
            granted_balance: info.granted_balance,
            topped_up_balance: info.topped_up_balance,
        })
    }

    #[tauri::command]
    fn save_usage_token(usage_token: String) -> Result<AppConfig, String> {
        let value = usage_token.trim().to_string();
        if value.is_empty() {
            return Err("用量 Token 不能为空".to_string());
        }
        if value.len() > 4096 {
            return Err("用量 Token 长度超出限制".to_string());
        }
        let mut config = read_stored_config()?;
        config.usage_token = Some(value);
        write_stored_config(&config)?;
        to_app_config(config)
    }

    #[tauri::command]
    fn clear_usage_token() -> Result<AppConfig, String> {
        let mut config = read_stored_config()?;
        config.usage_token = None;
        write_stored_config(&config)?;
        to_app_config(config)
    }

    const USAGE_TOKEN_TITLE_PREFIX: &str = "DSM_USAGE_TOKEN:";

    fn capture_usage_token(app: &tauri::AppHandle, token: String) -> Result<AppConfig, String> {
        let value = token.trim().to_string();
        if value.is_empty() {
            return Err("用量 Token 为空".to_string());
        }
        let mut config = read_stored_config()?;
        config.usage_token = Some(value);
        write_stored_config(&config)?;
        let app_config = to_app_config(config)?;

        // 标记本次同步已成功，避免 watcher 在窗口关闭后误发"结束等待"事件
        if let Some(flag) = app.try_state::<Arc<AtomicBool>>() {
            flag.store(true, Ordering::SeqCst);
        }

        if let Some(window) = app.get_webview_window("login-sync") {
            let _ = window.close();
        }

        let _ = app.emit("usage-token-captured", &app_config);

        Ok(app_config)
    }

    // 用 token 试调平台用量接口，验证它确实是有效的用量 token。
    //
    // 注意：平台在 token 缺失或失效时返回的是 HTTP 200 + `{"code":40002|40003}`，
    // 只看状态码会把无效 token 当成有效，所以这里必须解析响应体里的 code。
    async fn verify_usage_token(token: &str, month: u32, year: u32) -> Result<(), String> {
        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36";
        let url =
            format!("https://platform.deepseek.com/api/v0/usage/amount?month={month}&year={year}");
        let resp = reqwest::Client::new()
            .get(&url)
            .bearer_auth(token)
            .header("x-app-version", env!("CARGO_PKG_VERSION"))
            .header("Accept", "*/*")
            .header("User-Agent", ua)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|error| format!("验证 token 失败：{error}"))?;

        let status = resp.status().as_u16();
        let body = resp
            .json::<serde_json::Value>()
            .await
            .map_err(|error| format!("验证 token 失败：无法解析平台响应（{error}）"))?;

        if status != 200 {
            return Err(match status {
                401 => "token 无效或已过期，请重新登录同步".to_string(),
                429 => "请求过于频繁，请稍后再试".to_string(),
                code => format!("token 无效：HTTP {code}"),
            });
        }

        let code = body.get("code").and_then(|value| value.as_i64()).unwrap_or(-1);
        if code == 0 && body.get("data").is_some_and(|value| !value.is_null()) {
            return Ok(());
        }

        let msg = body
            .get("msg")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        Err(match code {
            40002 | 40003 => "token 无效或已失效，请重新登录同步".to_string(),
            _ if msg.is_empty() => format!("token 验证失败：平台返回 code {code}"),
            _ => format!("token 验证失败：{msg}（code {code}）"),
        })
    }

    fn read_shared_text(path: &Path) -> Option<String> {
        let mut file = fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1 | 0x2 | 0x4)
            .open(path)
            .ok()?;
        let metadata = file.metadata().ok()?;
        if metadata.len() == 0 || metadata.len() > 20 * 1024 * 1024 {
            return None;
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.read_to_end(&mut bytes).ok()?;
        Some(String::from_utf8_lossy(&bytes).replace('\0', ""))
    }

    fn extract_user_api_token(text: &str) -> Option<String> {
        let mut search_from = 0;
        let marker = "\"token\":\"";
        while let Some(relative_index) = text[search_from..].find(marker) {
            let token_start = search_from + relative_index + marker.len();
            let token_end = token_start + text[token_start..].find('"')?;
            let token = &text[token_start..token_end];
            let context_end = (token_end + 1800).min(text.len());
            let context = &text[token_end..context_end];
            if token.len() > 20
                && context.contains("\"id_profile\"")
                && context.contains("\"feature_gates\"")
            {
                return Some(token.to_string());
            }
            search_from = token_end + 1;
        }
        None
    }

    fn find_webview_cached_usage_token() -> Option<String> {
        let local_app_data = std::env::var_os("LOCALAPPDATA")?;
        let cache_dir = PathBuf::from(local_app_data)
            .join("com.deepseek.monitor.windows")
            .join("EBWebView")
            .join("Default")
            .join("Cache")
            .join("Cache_Data");
        let entries = fs::read_dir(cache_dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if let Some(text) = read_shared_text(&path) {
                if let Some(token) = extract_user_api_token(&text) {
                    return Some(token);
                }
            }
        }
        None
    }

    fn start_usage_title_watcher(app: tauri::AppHandle) {
        thread::spawn(move || {
            // 登录页加载并触发平台 API 请求需要时间，等待后再开始扫缓存
            thread::sleep(Duration::from_secs(3));
            for _ in 0..1200 {
                if let Some(token) = find_webview_cached_usage_token() {
                    let _ = capture_usage_token(&app, token);
                    return;
                }

                let Some(window) = app.get_webview_window("login-sync") else {
                    // 窗口已关闭：若不是因成功捕获而关闭，才通知前端结束等待
                    let captured = app
                        .try_state::<Arc<AtomicBool>>()
                        .map(|flag| flag.load(Ordering::SeqCst))
                        .unwrap_or(false);
                    if !captured {
                        let _ = app.emit("usage-sync-ended", ());
                    }
                    return;
                };

                if let Ok(title) = window.title() {
                    if let Some(rest) = title.strip_prefix(USAGE_TOKEN_TITLE_PREFIX) {
                        // 注入脚本写入的格式：{year}:{month}:{token}
                        let mut parts = rest.splitn(3, ':');
                        if let (Some(y), Some(m), Some(tok)) =
                            (parts.next(), parts.next(), parts.next())
                        {
                            if let (Ok(year), Ok(month)) = (y.parse::<u32>(), m.parse::<u32>()) {
                                let token = tok.to_string();
                                // 验证 token 真能调用用量接口，过滤登录中途的临时 token
                                let verified = tauri::async_runtime::block_on(
                                    verify_usage_token(&token, month, year),
                                );
                                if verified.is_ok() {
                                    let _ = capture_usage_token(&app, token);
                                    return;
                                }
                            }
                        }
                    }
                }

                thread::sleep(Duration::from_millis(1500));
            }
            // 30 分钟超时，若仍未成功则通知前端结束等待
            let captured = app
                .try_state::<Arc<AtomicBool>>()
                .map(|flag| flag.load(Ordering::SeqCst))
                .unwrap_or(false);
            if !captured {
                let _ = app.emit("usage-sync-ended", ());
            }
        });
    }

    // 在登录窗口注入，hook fetch / XMLHttpRequest，主动从平台 API 请求的
    // Authorization 头里抓 Bearer token。登录后页面自动调 API 即可即时捕获，
    // 不再依赖 WebView2 磁盘缓存的延迟落盘。
    const USAGE_SYNC_POLL_JS: &str = r#"
    (function() {
      if (window.__dsm_token_hook__) return;
      window.__dsm_token_hook__ = true;
      var done = false;
      var ALLOWED_HOSTS = ['platform.deepseek.com', 'api.deepseek.com'];

      function isAllowedHost(url) {
        try {
          var u = new URL(url, window.location.href);
          return ALLOWED_HOSTS.indexOf(u.hostname) !== -1;
        } catch (e) { return false; }
      }

      function deliver(token) {
        if (done) return;
        if (!token || typeof token !== 'string') return;
        token = token.trim();
        if (token.length < 20) return;
        var now = new Date();
        var y = now.getFullYear();
        var m = now.getMonth() + 1;
        try { document.title = 'DSM_USAGE_TOKEN:' + y + ':' + m + ':' + token; } catch (e) {}
      }

      function fromAuth(value) {
        if (!value) return;
        var m = /Bearer\s+(\S+)/i.exec(String(value));
        if (m && m[1]) deliver(m[1]);
      }

      var origFetch = window.fetch;
      if (typeof origFetch === 'function') {
        window.fetch = function(input, init) {
          try {
            var requestUrl = (typeof input === 'string') ? input :
                             (input && typeof input.url === 'string') ? input.url : '';
            if (init && typeof init.url === 'string') requestUrl = init.url;
            if (isAllowedHost(requestUrl)) {
              var headers = (init && init.headers) || (input && input.headers);
              if (headers) {
                if (typeof Headers !== 'undefined' && headers instanceof Headers) {
                  fromAuth(headers.get('authorization'));
                } else if (Array.isArray(headers)) {
                  for (var i = 0; i < headers.length; i++) {
                    if (headers[i] && String(headers[i][0]).toLowerCase() === 'authorization') {
                      fromAuth(headers[i][1]);
                    }
                  }
                } else if (typeof headers === 'object') {
                  for (var k in headers) {
                    if (k.toLowerCase() === 'authorization') fromAuth(headers[k]);
                  }
                }
              }
            }
          } catch (e) {}
          return origFetch.apply(this, arguments);
        };
      }

      var origSet = XMLHttpRequest.prototype.setRequestHeader;
      XMLHttpRequest.prototype.setRequestHeader = function(name, value) {
        try {
          if (name && String(name).toLowerCase() === 'authorization' && isAllowedHost(this._dsmUrl || '')) {
            fromAuth(value);
          }
        } catch (e) {}
        return origSet.apply(this, arguments);
      };

      var origOpen = XMLHttpRequest.prototype.open;
      XMLHttpRequest.prototype.open = function(method, url) {
        try { this._dsmUrl = url; } catch (e) {}
        return origOpen.apply(this, arguments);
      };
    })();
    "#;

    #[tauri::command]
    async fn start_usage_sync(app: tauri::AppHandle) -> Result<bool, String> {
        // 重置本次同步的成功标志
        if let Some(flag) = app.try_state::<Arc<AtomicBool>>() {
            flag.store(false, Ordering::SeqCst);
        }

        // 先扫一次缓存：登录完成后重复点击本命令，缓存落盘后即可命中
        if let Some(token) = find_webview_cached_usage_token() {
            capture_usage_token(&app, token)?;
            return Ok(true);
        }

        // 登录窗口已存在：刷新它，促使用量页重新请求接口、把响应写入缓存，
        // 用户随后再点一次本按钮即可命中。不重复弹新窗口、不死等。
        if app.get_webview_window("login-sync").is_some() {
            if let Some(window) = app.get_webview_window("login-sync") {
                let _ = window.eval("location.reload();");
            }
            return Ok(false);
        }

        let url = tauri::WebviewUrl::External("https://platform.deepseek.com".parse().unwrap());
        tauri::WebviewWindowBuilder::new(
            &app,
            "login-sync",
            url,
        )
        .title("DeepSeek 账号登录")
        .inner_size(480.0, 720.0)
        .min_inner_size(360.0, 480.0)
        .resizable(true)
        .center()
        .visible(true)
        .initialization_script(USAGE_SYNC_POLL_JS)
        .on_navigation(|url| {
            url.host_str()
                .is_some_and(|host| host == "platform.deepseek.com" || host == "api.deepseek.com")
        })
        .on_page_load(|window, payload| {
            if matches!(payload.event(), PageLoadEvent::Finished)
                && payload
                    .url()
                    .host_str()
                    .is_some_and(|host| host == "platform.deepseek.com")
            {
                let _ = window.eval(USAGE_SYNC_POLL_JS);
            }
        })
        .build()
        .map_err(|error| format!("打开登录窗口失败：{error}"))?;
        start_usage_title_watcher(app);
        Ok(false)
    }

    #[tauri::command]
    async fn usage_token_captured(
        app: tauri::AppHandle,
        token: String,
        month: u32,
        year: u32,
    ) -> Result<AppConfig, String> {
        let value = token.trim().to_string();
        if value.is_empty() {
            return Err("用量 Token 为空".to_string());
        }
        // 先验证再保存：拦截到的 token 可能是登录中途的临时 token，
        // 只有能真正调用用量接口的才接受
        verify_usage_token(&value, month, year).await?;
        capture_usage_token(&app, value)
    }

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct UsageModelSummary {
        key: String,
        name: String,
        total_tokens: u64,
        request_count: u64,
        cache_hit_tokens: u64,
        cache_miss_tokens: u64,
        response_tokens: u64,
        cost: f64,
    }

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct UsageDaySummary {
        date: String,
        flash_tokens: u64,
        flash_cache_hit: u64,
        flash_cache_miss: u64,
        flash_response: u64,
        pro_tokens: u64,
        pro_cache_hit: u64,
        pro_cache_miss: u64,
        pro_response: u64,
        vision_tokens: u64,
        vision_cache_hit: u64,
        vision_cache_miss: u64,
        vision_response: u64,
        total_tokens: u64,
        total_cost: f64,
    }

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct UsageResult {
        models: Vec<UsageModelSummary>,
        days: Vec<UsageDaySummary>,
        month_cost: f64,
    }

    // 通过 DeepSeek 平台内部接口拉取用量与费用（需网页登录 token，非官方 API Key）。
    #[tauri::command]
    async fn fetch_usage(month: u32, year: u32) -> Result<UsageResult, String> {
        let config = read_stored_config()?;
        let token = config
            .usage_token
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "未配置用量 Token".to_string())?;

        #[derive(Deserialize)]
        struct Entry {
            #[serde(rename = "type")]
            kind: String,
            amount: String,
        }
        #[derive(Deserialize)]
        struct ModelUsage {
            model: String,
            usage: Vec<Entry>,
        }
        #[derive(Deserialize)]
        struct DayUsage {
            date: String,
            data: Vec<ModelUsage>,
        }
        #[derive(Deserialize)]
        struct AmountBiz {
            total: Vec<ModelUsage>,
            days: Vec<DayUsage>,
        }
        #[derive(Deserialize)]
        struct AmountData {
            biz_data: AmountBiz,
        }
        #[derive(Deserialize)]
        struct AmountResp {
            data: AmountData,
        }
        #[derive(Deserialize)]
        struct CostBiz {
            total: Vec<ModelUsage>,
            days: Vec<DayUsage>,
        }
        #[derive(Deserialize)]
        struct CostData {
            biz_data: Vec<CostBiz>,
        }
        #[derive(Deserialize)]
        struct CostResp {
            data: CostData,
        }

        async fn get_json<T: serde::de::DeserializeOwned>(
            client: &reqwest::Client,
            url: &str,
            token: &str,
        ) -> Result<T, String> {
            let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                      (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36";
            let resp = client
                .get(url)
                .bearer_auth(token)
                .header("x-app-version", env!("CARGO_PKG_VERSION"))
                .header("Accept", "*/*")
                .header("User-Agent", ua)
                .timeout(std::time::Duration::from_secs(15))
                .send()
                .await
                .map_err(|error| format!("用量请求失败：{error}"))?;
            match resp.status().as_u16() {
                200 => {}
                401 => return Err("用量 Token 无效或已过期，请重新获取".to_string()),
                429 => return Err("请求过于频繁，请稍后再试".to_string()),
                code => return Err(format!("用量接口错误：HTTP {code}")),
            }

            // 平台在 token 失效时同样返回 HTTP 200，只是 body 里带非 0 的 code，
            // 这里先解析 code，避免直接反序列化 data 时报出难以理解的类型错误。
            let body = resp
                .json::<serde_json::Value>()
                .await
                .map_err(|error| format!("解析用量数据失败：{error}"))?;
            if let Some(code) = body.get("code").and_then(|value| value.as_i64()) {
                if code != 0 {
                    let msg = body
                        .get("msg")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    return Err(match code {
                        40002 | 40003 => {
                            "用量 Token 无效或已失效，请重新登录同步或重新粘贴".to_string()
                        }
                        _ if msg.is_empty() => format!("用量接口返回错误：code {code}"),
                        _ => format!("用量接口返回错误：{msg}（code {code}）"),
                    });
                }
            }

            serde_json::from_value::<T>(body).map_err(|error| format!("解析用量数据失败：{error}"))
        }

        fn token_breakdown(usage: &[Entry]) -> (u64, u64, u64, u64, u64) {
            // 返回 (总 token, 请求数, 缓存命中, 缓存未命中, 输出 token)
            let mut total = 0u64;
            let mut request = 0u64;
            let mut hit = 0u64;
            let mut miss = 0u64;
            let mut response = 0u64;
            for entry in usage {
                let value = entry.amount.parse::<f64>().unwrap_or(0.0).round() as u64;
                match entry.kind.as_str() {
                    "REQUEST" => request = value,
                    "PROMPT_CACHE_HIT_TOKEN" => {
                        hit = value;
                        total += value;
                    }
                    "PROMPT_CACHE_MISS_TOKEN" => {
                        miss = value;
                        total += value;
                    }
                    "RESPONSE_TOKEN" => {
                        response = value;
                        total += value;
                    }
                    "PROMPT_TOKEN" => total += value,
                    _ => {}
                }
            }
            (total, request, hit, miss, response)
        }

        fn cost_sum(usage: &[Entry]) -> f64 {
            usage
                .iter()
                .filter(|entry| entry.kind != "REQUEST")
                .map(|entry| entry.amount.parse::<f64>().unwrap_or(0.0))
                .sum()
        }

        let client = reqwest::Client::new();
        let amount_url =
            format!("https://platform.deepseek.com/api/v0/usage/amount?month={month}&year={year}");
        let cost_url =
            format!("https://platform.deepseek.com/api/v0/usage/cost?month={month}&year={year}");

        let amount: AmountResp = get_json(&client, &amount_url, &token).await?;
        let cost: CostResp = get_json(&client, &cost_url, &token).await?;

        let cost_total = cost.data.biz_data.first();
        let cost_for_model = |model: &str| -> f64 {
            cost_total
                .and_then(|item| item.total.iter().find(|m| m.model == model))
                .map(|m| cost_sum(&m.usage))
                .unwrap_or(0.0)
        };

        let mut models: Vec<UsageModelSummary> = Vec::new();
        for model_usage in &amount.data.biz_data.total {
            let Some((key, name)) = model_group(&model_usage.model) else {
                log::warn!(
                    "未识别的模型 ID「{}」：其用量会计入当月与当日总量，但不会单独成行展示",
                    model_usage.model
                );
                continue;
            };
            let (total, request, hit, miss, response) = token_breakdown(&model_usage.usage);
            let cost = cost_for_model(&model_usage.model);
            // 同一分组的多个历史 / 别名 ID 合并成一行。
            match models.iter_mut().find(|item| item.key == key) {
                Some(existing) => {
                    existing.total_tokens += total;
                    existing.request_count += request;
                    existing.cache_hit_tokens += hit;
                    existing.cache_miss_tokens += miss;
                    existing.response_tokens += response;
                    existing.cost += cost;
                }
                None => models.push(UsageModelSummary {
                    key: key.to_string(),
                    name: name.to_string(),
                    total_tokens: total,
                    request_count: request,
                    cache_hit_tokens: hit,
                    cache_miss_tokens: miss,
                    response_tokens: response,
                    cost,
                }),
            }
        }

        let mut cost_by_date: std::collections::HashMap<String, f64> =
            std::collections::HashMap::new();
        if let Some(item) = cost_total {
            for day in &item.days {
                let day_cost: f64 = day.data.iter().map(|m| cost_sum(&m.usage)).sum();
                cost_by_date.insert(day.date.clone(), day_cost);
            }
        }

        let mut days = Vec::new();
        for day in &amount.data.biz_data.days {
            let mut flash = 0u64;
            let mut flash_hit = 0u64;
            let mut flash_miss = 0u64;
            let mut flash_resp = 0u64;
            let mut pro = 0u64;
            let mut pro_hit = 0u64;
            let mut pro_miss = 0u64;
            let mut pro_resp = 0u64;
            let mut vision = 0u64;
            let mut vision_hit = 0u64;
            let mut vision_miss = 0u64;
            let mut vision_resp = 0u64;
            let mut total = 0u64;
            for model_usage in &day.data {
                let (tokens, _, hit, miss, response) = token_breakdown(&model_usage.usage);
                total += tokens;
                match model_group(&model_usage.model).map(|(key, _)| key) {
                    Some("flash") => {
                        flash += tokens;
                        flash_hit += hit;
                        flash_miss += miss;
                        flash_resp += response;
                    }
                    Some("pro") => {
                        pro += tokens;
                        pro_hit += hit;
                        pro_miss += miss;
                        pro_resp += response;
                    }
                    Some("flashvision") => {
                        vision += tokens;
                        vision_hit += hit;
                        vision_miss += miss;
                        vision_resp += response;
                    }
                    _ => {}
                }
            }
            days.push(UsageDaySummary {
                date: day.date.clone(),
                flash_tokens: flash,
                flash_cache_hit: flash_hit,
                flash_cache_miss: flash_miss,
                flash_response: flash_resp,
                pro_tokens: pro,
                pro_cache_hit: pro_hit,
                pro_cache_miss: pro_miss,
                pro_response: pro_resp,
                vision_tokens: vision,
                vision_cache_hit: vision_hit,
                vision_cache_miss: vision_miss,
                vision_response: vision_resp,
                total_tokens: total,
                total_cost: cost_by_date.get(&day.date).copied().unwrap_or(0.0),
            });
        }

        let month_cost: f64 = cost_total
            .map(|item| item.total.iter().map(|m| cost_sum(&m.usage)).sum())
            .unwrap_or(0.0);

        Ok(UsageResult {
            models,
            days,
            month_cost,
        })
    }

    tauri::Builder::default()
        // 单实例守卫：必须作为第一个注册的插件。
        // 程序已运行时再次启动 exe，第二个进程不会新开窗口，
        // 而是触发此回调把已有主窗口显示并聚焦，随后第二个进程自行退出。
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                show_main_window(&window);
            }
        }))
        .manage(Arc::new(AtomicBool::new(false)))
        .invoke_handler(tauri::generate_handler![
            hide_main_window,
            set_native_material,
            get_app_config,
            save_api_key,
            clear_api_key,
            save_refresh_interval,
            save_auto_refresh_enabled,
            save_autostart,
            fetch_balance,
            save_usage_token,
            clear_usage_token,
            fetch_usage,
            start_usage_sync,
            usage_token_captured
        ])
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            let show_item = MenuItem::with_id(app, "show", "显示主面板", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&show_item, &quit_item])?;

            let mut tray_builder = TrayIconBuilder::new()
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            show_main_window(&window);
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    // 仅在左键“抬起”时切换；否则按下+抬起各触发一次，窗口会闪现后立即隐藏
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let is_visible = window.is_visible().unwrap_or(false);
                            if is_visible {
                                let _ = window.hide();
                            } else {
                                show_main_window(&window);
                            }
                        }
                    }
                });

            if let Some(icon) = app.default_window_icon() {
                tray_builder = tray_builder.icon(icon.clone());
            }

            tray_builder.build(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
