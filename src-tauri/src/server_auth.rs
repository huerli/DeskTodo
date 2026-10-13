//! 登录密码保存在系统凭据库；askpass 子进程只向 OpenSSH 的专用管道输出密码。
use crate::model::ServerSettings;
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
use std::process::Stdio;
use crate::process::background_command;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

const PASSWORD_SERVICE: &str = "com.desktodo.app.ssh-password";
static VAULT_LOCK: Mutex<()> = Mutex::new(());

fn endpoint(server: &ServerSettings) -> Result<String, String> {
    let host = server.host.trim().trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() || host.starts_with('-') || server.port == 0
        || !host.chars().all(|c| c.is_ascii_alphanumeric() || ".-:".contains(c))
    { return Err("请填写有效的服务器主机名 / IP 和 SSH 端口".into()); }
    Ok(host.into())
}

pub fn password_account(server: &ServerSettings) -> Result<String, String> {
    let host = endpoint(server)?;
    let user = server.username.trim();
    if user.is_empty() || user.starts_with('-')
        || !user.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
    { return Err("请填写有效的 SSH 用户名".into()); }
    Ok(format!("{user}@{host}:{}", server.port))
}

fn vault_error(_: keyring::Error) -> String {
    "无法访问系统密钥管理器，请解锁系统凭据库后重试".into()
}

pub fn save_password(server: &ServerSettings, password: String) -> Result<(), String> {
    let password = Zeroizing::new(password);
    if password.is_empty() || password.chars().any(|c| matches!(c, '\0' | '\r' | '\n')) {
        return Err("请输入有效的 SSH 登录密码".into());
    }
    let _lock = VAULT_LOCK.lock().map_err(|_| "密码保存锁异常")?;
    let entry = keyring::Entry::new(PASSWORD_SERVICE, &password_account(server)?).map_err(vault_error)?;
    entry.set_password(&password).map_err(vault_error)
}

pub fn ensure_password(server: &ServerSettings) -> Result<String, String> {
    let account = password_account(server)?;
    let _lock = VAULT_LOCK.lock().map_err(|_| "密码读取锁异常")?;
    let entry = keyring::Entry::new(PASSWORD_SERVICE, &account).map_err(vault_error)?;
    let password = Zeroizing::new(entry.get_password().map_err(|error| match error {
        keyring::Error::NoEntry => "尚未保存此服务器的登录密码，请在密码栏填写后重试".into(),
        error => vault_error(error),
    })?);
    if password.is_empty() { return Err("已保存的 SSH 登录密码为空".into()); }
    Ok(account)
}

pub fn askpass_main() -> i32 {
    let prompt = std::env::args().nth(1).unwrap_or_default().to_ascii_lowercase();
    // 绝不代替用户同意主机指纹，也不把密码当作验证码或私钥口令。
    if !prompt.contains("password") { return 1; }
    let Ok(account) = std::env::var("DESK_TODO_SSH_PASSWORD_ENTRY") else { return 1; };
    let Ok(entry) = keyring::Entry::new(PASSWORD_SERVICE, &account) else { return 1; };
    let Ok(password) = entry.get_password() else { return 1; };
    let password = Zeroizing::new(password);
    let mut output = std::io::stdout().lock();
    if output.write_all(password.as_bytes()).is_err() || output.write_all(b"\n").is_err() { return 1; }
    0
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostPreview {
    pub host: String,
    pub port: u16,
    pub key_type: String,
    pub fingerprint: String,
    pub confirmation_id: String,
}
struct PendingHost { preview: HostPreview, key_line: String, scanned_at: Instant }
static PENDING_HOST: Mutex<Option<PendingHost>> = Mutex::new(None);

fn supported_host_key(key_type: &str) -> bool {
    matches!(key_type, "ssh-ed25519" | "ecdsa-sha2-nistp256" | "ecdsa-sha2-nistp384" | "ecdsa-sha2-nistp521" | "ssh-rsa")
}

fn scanned_key(stdout: &str, host: &str, port: u16) -> Option<String> {
    let expected = if port == 22 { host.to_string() } else { format!("[{host}]:{port}") };
    // Some clients bracket IPv6 even for the default port. Save a canonical
    // endpoint so pairing and subsequent trust checks use the same spelling.
    let bracketed = format!("[{host}]:{port}");
    ["ssh-ed25519", "ecdsa-sha2-nistp256", "ecdsa-sha2-nistp384", "ecdsa-sha2-nistp521", "ssh-rsa"]
        .iter().find_map(|key_type| {
            stdout.lines().find_map(|line| {
                let fields: Vec<_> = line.split_whitespace().collect();
                if fields.len() == 3 && (fields[0] == expected || fields[0] == bracketed) && fields[1] == *key_type {
                    Some(format!("{expected} {} {}", fields[1], fields[2]))
                } else { None }
            })
        })
}

fn scan_error(stderr: &str, host: &str, port: u16) -> String {
    let details = stderr.lines().filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .take(6).collect::<Vec<_>>().join("\n");
    let message = format!("未读取到服务器 {host}:{port} 的主机密钥（ED25519 / ECDSA / RSA），请检查 SSH 地址、端口及服务器主机密钥配置");
    if details.is_empty() { message } else { format!("{message}\nSSH 诊断：{details}") }
}

#[cfg(windows)]
fn git_keyscan(git: &Path) -> Option<PathBuf> {
    // Git may be on PATH via cmd/, bin/, or mingw64/bin/. Keep the
    // scanner alongside its MSYS runtime instead of copying the executable.
    git.ancestors().skip(1).take(3)
        .map(|directory| directory.join("usr/bin/ssh-keyscan.exe"))
        .find(|path| path.is_file())
}

fn scan_keys(host: &str, port: u16) -> Result<String, String> {
    let scan = which::which("ssh-keyscan").map_err(|_| "未找到 ssh-keyscan，请安装 OpenSSH 客户端")?;
    let run = |binary: &Path| background_command(binary)
        .args(["-T", "5", "-p", &port.to_string(), "-t", "ed25519,ecdsa,rsa", host])
        .output().map_err(|e| format!("读取服务器指纹失败：{e}"));
    let output = run(&scan)?;
    if let Some(key) = scanned_key(&String::from_utf8_lossy(&output.stdout), host, port) {
        return Ok(key);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    #[cfg(windows)]
    if stderr.contains("choose_kex: unsupported KEX method") {
        // Win32-OpenSSH #2140: older keyscan builds advertise a KEX they
        // cannot implement. ssh.exe itself is unaffected; only retry scanning.
        if let Some(alternative) = which::which("git").ok().and_then(|git| git_keyscan(&git))
            .filter(|path| path != &scan)
        {
            let retried = run(&alternative)?;
            return scanned_key(&String::from_utf8_lossy(&retried.stdout), host, port)
                .ok_or_else(|| scan_error(&String::from_utf8_lossy(&retried.stderr), host, port));
        }
        return Err(format!("Windows OpenSSH 的主机密钥扫描工具存在密钥交换兼容问题，请更新 OpenSSH 或安装 Git for Windows 后重试。\n{}", scan_error(&stderr, host, port)));
    }
    Err(scan_error(&stderr, host, port))
}

pub fn scan_host(server: &ServerSettings) -> Result<HostPreview, String> {
    *PENDING_HOST.lock().map_err(|_| "主机指纹锁异常")? = None;
    let host = endpoint(server)?;
    let key_line = scan_keys(&host, server.port)?;
    let key_type = key_line.split_whitespace().nth(1).unwrap().to_string();
    let keygen = which::which("ssh-keygen").map_err(|_| "未找到 ssh-keygen，请安装 OpenSSH 客户端")?;
    let mut child = background_command(keygen).args(["-l", "-f", "-", "-E", "sha256"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?;
    writeln!(child.stdin.take().ok_or("无法计算主机指纹")?, "{key_line}").map_err(|e| e.to_string())?;
    let result = child.wait_with_output().map_err(|e| e.to_string())?;
    if !result.status.success() { return Err("服务器主机密钥格式不正确，无法计算指纹".into()); }
    let fingerprint = String::from_utf8_lossy(&result.stdout).split_whitespace().nth(1)
        .filter(|value| value.starts_with("SHA256:")).ok_or("服务器主机密钥格式不正确")?.to_string();
    let preview = HostPreview { host, port: server.port, key_type, fingerprint, confirmation_id: uuid::Uuid::new_v4().to_string() };
    *PENDING_HOST.lock().map_err(|_| "主机指纹锁异常")? = Some(PendingHost { preview: preview.clone(), key_line, scanned_at: Instant::now() });
    Ok(preview)
}

pub fn trust_host(dir: &Path, confirmation_id: &str) -> Result<String, String> {
    let mut pending = PENDING_HOST.lock().map_err(|_| "主机指纹锁异常")?;
    let preview = pending.as_ref().ok_or("请先读取并核对服务器指纹")?;
    if preview.preview.confirmation_id != confirmation_id || preview.scanned_at.elapsed() > Duration::from_secs(600) {
        return Err("指纹确认已失效，请重新读取服务器指纹".into());
    }
    let path = dir.join("server-known-hosts");
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => return Err("无法读取已保存的服务器指纹".into()),
    };
    let host_field = preview.key_line.split_whitespace().next().unwrap_or_default();
    for line in existing.lines().filter(|line| line.split_whitespace().next() == Some(host_field)) {
        if line.trim() != preview.key_line {
            return Err("已有服务器指纹与此次不同，已拒绝覆盖。请先确认服务器是否重装或连接是否被拦截".into());
        }
    }
    if !existing.lines().any(|line| line.trim() == preview.key_line) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let mut options = OpenOptions::new();
        options.append(true).create(true);
        #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        let mut file = options.open(path).map_err(|e| e.to_string())?;
        if !existing.is_empty() && !existing.ends_with('\n') { file.write_all(b"\n").map_err(|e| e.to_string())?; }
        writeln!(file, "{}", preview.key_line).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
    }
    *pending = None;
    Ok("服务器指纹已保存，可测试连接".into())
}

pub fn paired_host_key(dir:&Path,server:&ServerSettings)->Option<String> {
    let host=endpoint(server).ok()?;
    let expected=if server.port==22 {host}else{format!("[{host}]:{}",server.port)};
    std::fs::read_to_string(dir.join("server-known-hosts")).ok()?.lines()
        .find(|line|line.split_whitespace().next()==Some(expected.as_str())).map(str::to_string)
}
pub fn trust_paired_host(dir:&Path,server:&ServerSettings,line:&str)->Result<(),String> {
    let host=endpoint(server)?;
    let expected=if server.port==22 {host.clone()}else{format!("[{host}]:{}",server.port)};
    let fields:Vec<_>=line.split_whitespace().collect();
    if fields.len()!=3 || fields[0]!=expected || !supported_host_key(fields[1]) {return Err("配对码中的服务器指纹信息不正确".into());}
    let token=uuid::Uuid::new_v4().to_string();
    let preview=HostPreview {host,port:server.port,key_type:fields[1].into(),fingerprint:"paired".into(),confirmation_id:token.clone()};
    *PENDING_HOST.lock().map_err(|_|"主机指纹锁异常")?=Some(PendingHost {preview,key_line:line.trim().into(),scanned_at:Instant::now()});
    trust_host(dir,&token).map(|_|())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(windows)]
    fn discovers_git_scanner_from_common_git_entrypoints() {
        let root = std::env::temp_dir().join(format!("desk-todo-scanner-test-{}", uuid::Uuid::new_v4()));
        let scanner = root.join("usr/bin/ssh-keyscan.exe");
        assert!(git_keyscan(&root.join("cmd/git.exe")).is_none());
        std::fs::create_dir_all(scanner.parent().unwrap()).unwrap();
        std::fs::write(&scanner, []).unwrap();
        for entrypoint in ["cmd/git.exe", "bin/git.exe", "mingw64/bin/git.exe"] {
            assert_eq!(git_keyscan(&root.join(entrypoint)), Some(scanner.clone()));
        }
        std::fs::remove_file(&scanner).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    #[ignore = "requires DESK_TODO_TEST_SSH_HOST and a reachable SSH server"]
    fn live_host_scan_without_trusting_or_logging_in() {
        let server = ServerSettings {
            host: std::env::var("DESK_TODO_TEST_SSH_HOST").expect("set DESK_TODO_TEST_SSH_HOST"),
            port: std::env::var("DESK_TODO_TEST_SSH_PORT").unwrap_or_else(|_| "22".into()).parse().unwrap(),
            ..Default::default()
        };
        let preview = scan_host(&server).unwrap();
        assert!(preview.fingerprint.starts_with("SHA256:"));
        println!("{}:{} {} {}", preview.host, preview.port, preview.key_type, preview.fingerprint);
        *PENDING_HOST.lock().unwrap() = None;
    }

    #[test]
    fn scan_accepts_fallback_keys_and_prefers_ed25519() {
        let rsa = "example.com ssh-rsa RSAKEY";
        let ecdsa = "example.com ecdsa-sha2-nistp256 ECDSAKEY";
        let ed25519 = "example.com ssh-ed25519 EDKEY";
        assert_eq!(scanned_key(rsa, "example.com", 22).as_deref(), Some(rsa));
        assert_eq!(scanned_key(&format!("{rsa}\n{ecdsa}"), "example.com", 22).as_deref(), Some(ecdsa));
        assert_eq!(scanned_key(&format!("# banner\n{rsa}\n{ecdsa}\n{ed25519}\n"), "example.com", 22).as_deref(), Some(ed25519));
        assert!(supported_host_key("ssh-rsa"));
        assert!(supported_host_key("ecdsa-sha2-nistp521"));
        assert!(!supported_host_key("ssh-dss"));
    }

    #[test]
    fn scan_requires_requested_endpoint_and_supported_algorithm() {
        let line = "[example.com]:2222 ssh-ed25519 KEY";
        assert_eq!(scanned_key(line, "example.com", 2222).as_deref(), Some(line));
        assert!(scanned_key(line, "example.com", 22).is_none());
        assert!(scanned_key("other.com ssh-ed25519 KEY", "example.com", 22).is_none());
        assert!(scanned_key("example.com ssh-dss KEY", "example.com", 22).is_none());
        assert!(scanned_key("example.com ssh-ed25519 KEY extra", "example.com", 22).is_none());
        assert_eq!(scanned_key("[2001:db8::1]:22 ssh-ed25519 KEY", "2001:db8::1", 22).as_deref(), Some("2001:db8::1 ssh-ed25519 KEY"));
    }

    #[test]
    fn scan_failure_preserves_diagnostic_without_server_banner() {
        let error = scan_error("# host SSH-2.0-OpenSSH\nConnection refused\n", "example.com", 2222);
        assert!(error.contains("example.com:2222"));
        assert!(error.contains("Connection refused"));
        assert!(!error.contains("SSH-2.0-OpenSSH"));
        assert!(!scan_error("# banner\n", "example.com", 22).contains("SSH 诊断"));
    }

    #[test]
    fn rejects_option_injection_in_login_account() {
        let mut server = ServerSettings::default();
        server.host = "154.36.178.147".into();
        server.username = "root".into();
        assert_eq!(password_account(&server).unwrap(), "root@154.36.178.147:22");
        server.username = "-oProxyCommand=bad".into();
        assert!(password_account(&server).is_err());
    }

    #[test]
    fn host_trust_requires_review_token_and_preserves_existing_keys() {
        let dir = std::env::temp_dir().join(format!("desk-todo-host-test-{}", uuid::Uuid::new_v4()));
        let preview = HostPreview { host: "example.com".into(), port: 22, key_type: "ssh-ed25519".into(), fingerprint: "SHA256:test".into(), confirmation_id: "review-token".into() };
        let key_line = "example.com ssh-ed25519 TESTKEY".to_string();
        *PENDING_HOST.lock().unwrap() = Some(PendingHost { preview: preview.clone(), key_line: key_line.clone(), scanned_at: Instant::now() });
        assert!(trust_host(&dir, "unreviewed-token").is_err());
        assert!(!dir.exists());
        trust_host(&dir, "review-token").unwrap();
        let saved = std::fs::read_to_string(dir.join("server-known-hosts")).unwrap();
        assert_eq!(saved.trim(), key_line);
        *PENDING_HOST.lock().unwrap() = Some(PendingHost { preview, key_line: "example.com ssh-ed25519 CHANGEDKEY".into(), scanned_at: Instant::now() });
        assert!(trust_host(&dir, "review-token").is_err());
        assert_eq!(std::fs::read_to_string(dir.join("server-known-hosts")).unwrap(), saved);
        *PENDING_HOST.lock().unwrap() = None;
        std::fs::remove_dir_all(dir).unwrap();
    }
}
