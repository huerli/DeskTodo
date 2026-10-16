//! 通过系统 OpenSSH 将客户端密文写入服务器 SQLite。
use crate::model::{ServerSettings, SshAuthMethod};
#[cfg(test)]
use crate::model::{AppData,Settings};
use crate::server_auth;
use crate::process::background_command;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn remote_directory(settings: &ServerSettings) -> Result<String, String> {
    let path = settings.remote_dir.trim();
    if path.is_empty() || path.chars().any(char::is_control) {
        return Err("请填写有效的服务器保存目录".into());
    }
    // SSH 默认在登录用户的家目录执行；~ 必须在引用之外展开。
    Ok(if path == "~" {
        "\"$HOME\"".into()
    } else if let Some(rest) = path.strip_prefix("~/") {
        format!("\"$HOME\"/{}", quote(rest))
    } else {
        quote(path)
    })
}

fn ssh_command(settings: &ServerSettings) -> Result<Command, String> {
    let host = settings.host.trim().trim_start_matches('[').trim_end_matches(']');
    let user = settings.username.trim();
    if host.is_empty() || host.starts_with('-')
        || !host.chars().all(|c| c.is_ascii_alphanumeric() || ".-:".contains(c))
    {
        return Err("服务器地址请填写主机名或 IP，不包含 ssh://、用户名或端口".into());
    }
    if user.is_empty() || user.starts_with('-')
        || !user.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
    {
        return Err("请填写有效的 SSH 用户名".into());
    }
    if settings.port == 0 {
        return Err("SSH 端口必须为 1–65535".into());
    }
    remote_directory(settings)?;
    let binary = which::which("ssh").map_err(|_| "未找到 ssh，请先安装 OpenSSH 客户端".to_string())?;
    let mut command = background_command(binary);
    command.args([
        "-T", "-o", "StrictHostKeyChecking=yes",
        "-o", "ConnectTimeout=10", "-o", "ServerAliveInterval=10",
        "-o", "ServerAliveCountMax=2", "-p", &settings.port.to_string(),
    ]);
    if settings.known_hosts_path.is_file() {
        let file = settings.known_hosts_path.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
        command.arg("-o").arg(format!("UserKnownHostsFile=\"{file}\" ~/.ssh/known_hosts"));
    }
    if settings.auth_method == SshAuthMethod::Password {
        let account = server_auth::ensure_password(settings)?;
        command.args(["-o", "BatchMode=no", "-o", "PreferredAuthentications=password,keyboard-interactive",
            "-o", "PubkeyAuthentication=no", "-o", "NumberOfPasswordPrompts=1"]);
        command.env("SSH_ASKPASS", std::env::current_exe().map_err(|e| e.to_string())?)
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env("DESK_TODO_SSH_ASKPASS", "1")
            .env("DESK_TODO_SSH_PASSWORD_ENTRY", account);
    } else {
        command.args(["-o", "BatchMode=yes"]);
    }
    let key = settings.ssh_key_path.trim();
    if settings.auth_method == SshAuthMethod::Key && !key.is_empty() {
        let path = if let Some(rest) = key.strip_prefix("~/") {
            dirs::home_dir().ok_or("无法找到用户目录")?.join(rest)
        } else {
            PathBuf::from(key)
        };
        if !path.is_file() {
            return Err("SSH 私钥栏需要填写现有私钥文件的完整路径，不能填写登录密码。若使用密码，请选择密码登录".into());
        }
        command.arg("-i").arg(path);
    }
    command.arg("--").arg(format!("{user}@{host}"));
    Ok(command)
}

const DATABASE_SCRIPT: &str = include_str!("server/database.py");

fn database_name(settings: &ServerSettings) -> Result<&str, String> {
    let name = settings.database_name.trim();
    if name.is_empty() || name == "." || name == ".."
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
    {
        return Err("数据库文件名只能包含英文字母、数字、点、横线和下划线".into());
    }
    Ok(name)
}

fn database_command(settings: &ServerSettings, mode: &str, device: Option<uuid::Uuid>) -> Result<String, String> {
    let directory = remote_directory(settings)?;
    let name = database_name(settings)?;
    let prepare = if mode == "upload" || mode == "cas" {
        "mkdir -p -- \"$d\" && cd -- \"$d\" || exit 1; ".to_string()
    } else if mode == "fetch" {
        // 尚未创建目录时也允许读取空清单，Python 读取时不会创建文件。
        "".to_string()
    } else if mode == "check" {
        "if [ -e \"$d\" ]; then [ -d \"$d\" ] && [ -w \"$d\" ] && [ -x \"$d\" ] || exit 1; \
         else p=$d; while [ ! -e \"$p\" ]; do p=$(dirname -- \"$p\"); done; \
         [ -d \"$p\" ] && [ -w \"$p\" ] && [ -x \"$p\" ] || exit 1; fi; ".to_string()
    } else {
        "cd -- \"$d\" || exit 1; ".to_string()
    };
    let database = if mode == "check" || mode == "fetch" { format!("\"$d\"/{}", quote(name)) } else { quote(name) };
    let identity = device.map(|id| format!(" {}", quote(&id.to_string()))).unwrap_or_default();
    Ok(format!("umask 077; d={directory}; {prepare}python3 -c {} {} {database}{identity}", quote(DATABASE_SCRIPT), quote(mode)))
}

struct PrivateTemp {
    path: PathBuf,
    file: Option<File>,
}
impl PrivateTemp {
    fn new() -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!("desk-todo-{}.tmp", uuid::Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path).map_err(|e| format!("创建上传临时文件失败：{e}"))?;
        Ok(Self { path, file: Some(file) })
    }
}
impl Drop for PrivateTemp {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
    }
}

fn execute(mut command: Command, input: &[u8]) -> Result<Vec<u8>, String> {
    let mut payload = PrivateTemp::new()?;
    payload.file.as_mut().unwrap().write_all(input).map_err(|e| e.to_string())?;
    let errors = PrivateTemp::new()?;
    let output = PrivateTemp::new()?;
    let mut child = command
        .stdin(File::open(&payload.path).map_err(|e| e.to_string())?)
        .stdout(output.file.as_ref().unwrap().try_clone().map_err(|e| e.to_string())?)
        .stderr(errors.file.as_ref().unwrap().try_clone().map_err(|e| e.to_string())?)
        .spawn().map_err(|e| format!("启动 SSH 失败：{e}"))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < Duration::from_secs(60) => {
                std::thread::sleep(Duration::from_millis(50));
            }
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(match result {
                    Err(e) => format!("等待 SSH 失败：{e}"),
                    _ => "SSH 操作超时（60 秒），请检查网络和服务器".into(),
                });
            }
        }
    };
    if !status.success() {
        let error = std::fs::read_to_string(&errors.path).unwrap_or_default();
        if error.contains("Host key verification failed") || error.contains("REMOTE HOST IDENTIFICATION HAS CHANGED") {
            return Err("服务器主机指纹尚未确认或已发生变化。请点击「确认服务器指纹」，与服务器后台的指纹核对一致后保存，再测试连接".into());
        }
        if error.contains("Permission denied") {
            return Err("SSH 登录认证失败，请检查用户名、登录方式和密码 / 私钥。服务器还需允许该用户登录".into());
        }
        return Err(format!("SSH 操作失败：{}", if error.trim().is_empty() { status.to_string() } else { error.trim().to_string() }));
    }
    if std::fs::metadata(&output.path).map_err(|e| e.to_string())?.len() > 64 * 1024 * 1024 {
        return Err("服务器返回数据过大，已停止读取".into());
    }
    std::fs::read(&output.path).map_err(|e| e.to_string())
}

pub fn test_connection(settings: &ServerSettings) -> Result<String, String> {
    let mut command = ssh_command(settings)?;
    command.arg(database_command(settings, "check", None)?);
    execute(command, &[])?;
    Ok("SSH 连接成功，Python 3 / SQLite 可用，数据库位置可写".into())
}

#[cfg(test)]
pub fn upload_payload(mut data: AppData) -> Result<Vec<u8>, String> {
    // 上传可导入的数据，不向服务器传输认证信息、窗口位置或本机目录。
    data.settings = Settings::default();
    data.server_sync = None;
    data.data_dir.clear();
    data.notices.clear();
    data.last_sync = Some(crate::model::now_rfc3339());
    serde_json::to_vec_pretty(&data).map_err(|e| e.to_string())
}


pub fn download(settings: &ServerSettings, device: uuid::Uuid) -> Result<Vec<u8>, String> {
    let mut command = ssh_command(settings)?;
    command.arg(database_command(settings, "download", Some(device))?);
    execute(command, &[])
}

pub fn fetch(settings:&ServerSettings,group:uuid::Uuid) -> Result<(u64,Option<Vec<u8>>),String> {
    let mut command=ssh_command(settings)?;
    command.arg(database_command(settings,"fetch",Some(group))?);
    let response=execute(command,&[])?;
    if response.len()<8 {return Err("服务器同步响应不完整".into());}
    let revision=u64::from_le_bytes(response[..8].try_into().map_err(|_|"服务器版本响应不正确")?);
    if revision==0 && response.len()==8 {Ok((0,None))}
    else if revision>0 && response.len()>8 {Ok((revision,Some(response[8..].to_vec())))}
    else {Err("服务器同步版本与数据不匹配".into())}
}
pub fn compare_and_swap(settings:&ServerSettings,revision:u64,encrypted:&[u8]) -> Result<bool,String> {
    let mut command=ssh_command(settings)?;
    command.arg(database_command(settings,"cas",None)?);
    let mut request=revision.to_le_bytes().to_vec();request.extend_from_slice(encrypted);
    match execute(command,&request)?.as_slice() { [1]=>Ok(true),[0]=>Ok(false),_=>Err("服务器写入确认响应不正确".into()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn settings(directory: String) -> ServerSettings {
        ServerSettings { host: "example.com".into(), username: "todo".into(), remote_dir: directory, ..Default::default() }
    }

    #[test]
    fn exported_snapshot_omits_credentials_and_local_paths() {
        let mut data = AppData::default();
        data.settings.sync.token = "secret-token".into();
        data.settings.server.ssh_key_path = "/private/key".into();
        data.settings.daily_summary.export_dir = "/private/reports".into();
        data.data_dir = "/private/data".into();
        let original_todo: crate::model::Todo = serde_json::from_str(r#"{"id":"test","title":"保留待办"}"#).unwrap();
        data.todos.push(original_todo.clone());
        data.trash.push(original_todo);
        let payload = String::from_utf8(upload_payload(data).unwrap()).unwrap();
        for secret in ["secret-token", "/private/key", "/private/reports", "/private/data"] {
            assert!(!payload.contains(secret));
        }
        let decoded: AppData = serde_json::from_str(&payload).unwrap();
        assert_eq!(decoded.todos.len(), 1);
        assert_eq!(decoded.trash.len(), 1);
        assert_eq!(decoded.todos[0].title, "保留待办");
    }

    #[test]
    fn old_settings_default_to_git() {
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.sync_method, crate::model::SyncMethod::Git);
        assert_eq!(settings.server.port, 22);
        assert!(!settings.server.auto_upload);
    }

    #[test]
    fn rejects_ssh_options_and_invalid_paths() {
        for host in ["-oProxyCommand=bad", "ssh://example.com", "user@host", "host;touch bad"] {
            let mut config = settings("desk-todo".into());
            config.host = host.into();
            assert!(ssh_command(&config).is_err());
        }
        assert!(remote_directory(&settings("bad\npath".into())).is_err());
        assert!(remote_directory(&settings(String::new())).is_err());
        let mut config = settings("desk-todo".into());
        config.ssh_key_path = "this-is-a-password-not-a-file".into();
        let error = ssh_command(&config).err().unwrap();
        assert!(error.contains("不能填写登录密码"));
        assert!(!error.contains(&config.ssh_key_path));
    }

    #[test]
    #[cfg(unix)]
    fn encrypted_database_round_trip_and_failed_input() {
        let root = std::env::temp_dir().join(format!("desk-todo-db-test-{}", uuid::Uuid::new_v4()));
        let directory = root.join("space ' $(touch INJECTED) ; dir");
        let config = settings(directory.to_string_lossy().into_owned());
        let key = crate::server_crypto::ServerKey::for_test();
        let device = key.id;
        let plaintext = b"PRIVATE-TASK-MARKER-not-readable-in-database";
        let encrypted = key.encrypt(plaintext).unwrap();
        let mut command = Command::new("sh");
        command.arg("-c").arg(database_command(&config, "check", None).unwrap());
        execute(command, &[]).unwrap();
        assert!(!root.exists());
        let mut command = Command::new("sh");
        command.arg("-c").arg(database_command(&config, "upload", None).unwrap());
        execute(command, &encrypted).unwrap();
        let mut command = Command::new("sh");
        command.arg("-c").arg(database_command(&config, "upload", None).unwrap());
        assert!(execute(command, b"partial invalid payload").is_err());
        let mut command = Command::new("sh");
        command.arg("-c").arg(database_command(&config, "download", Some(device)).unwrap());
        let downloaded = execute(command, &[]).unwrap();
        assert_eq!(downloaded, encrypted);
        assert_eq!(&*key.decrypt(&downloaded).unwrap(), plaintext);
        let other = crate::server_crypto::ServerKey::for_test();
        let other_payload = other.encrypt(b"SECOND DEVICE").unwrap();
        let mut command = Command::new("sh");
        command.arg("-c").arg(database_command(&config, "upload", None).unwrap());
        execute(command, &other_payload).unwrap();
        let mut command = Command::new("sh");
        command.arg("-c").arg(database_command(&config, "download", Some(device)).unwrap());
        assert_eq!(execute(command, &[]).unwrap(), encrypted);
        let mut command = Command::new("sh");
        command.arg("-c").arg(database_command(&config, "download", Some(other.id)).unwrap());
        assert_eq!(execute(command, &[]).unwrap(), other_payload);
        let database = std::fs::read(directory.join("desk-todo.sqlite3")).unwrap();
        assert!(!database.windows(plaintext.len()).any(|bytes| bytes == plaintext));
        assert!(directory.join("desk-todo.sqlite3").is_file());
        assert!(!directory.join("todos.json").exists());
        assert!(!root.join("INJECTED").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[cfg(unix)]
    fn two_devices_retry_stale_writes_and_converge() {
        use crate::sync_document::{Document,decode};
        let root=std::env::temp_dir().join(format!("desk-todo-two-devices-{}",uuid::Uuid::new_v4()));
        let config=settings(root.to_string_lossy().into_owned());
        let key=crate::server_crypto::ServerKey::for_test();
        let call=|mode:&str,input:&[u8]| {let mut command=Command::new("sh");command.arg("-c").arg(database_command(&config,mode,Some(key.id)).unwrap());execute(command,input).unwrap()};
        let fetch=|| {let bytes=call("fetch",&[]);(u64::from_le_bytes(bytes[..8].try_into().unwrap()),if bytes.len()>8 {decode(&key.decrypt(&bytes[8..]).unwrap()).unwrap()} else {Document {schema:1,..Default::default()}})};
        let cas=|revision:u64,document:&Document| {let mut bytes=revision.to_le_bytes().to_vec();bytes.extend(key.encrypt(&serde_json::to_vec(document).unwrap()).unwrap());call("cas",&bytes)==[1]};
        let mut a=AppData::default();a.todos.push(serde_json::from_str(r#"{"id":"one","title":"base","notes":"old","order":10}"#).unwrap());
        let (rev,remote)=fetch();let mut da=Document::bootstrap(&a,&remote,"A").unwrap();assert!(cas(rev,&da));
        let (old_revision,mut db)=fetch();let mut b=AppData::default();db.apply(&mut b).unwrap();assert_eq!(b.todos.len(),1);
        a.todos[0].title="edited on A".into();da.observe(&a,"A").unwrap();
        b.todos[0].notes="edited on B".into();b.todos.push(serde_json::from_str(r#"{"id":"two","title":"added on B","order":20}"#).unwrap());db.observe(&b,"B").unwrap();
        assert!(cas(old_revision,&da));assert!(!cas(old_revision,&db));
        let (revision,remote)=fetch();db.merge(&remote).unwrap();assert!(cas(revision,&db));
        let (revision,remote)=fetch();da.merge(&remote).unwrap();da.apply(&mut a).unwrap();assert_eq!(a.todos.len(),2);assert_eq!(a.todos[0].title,"edited on A");assert_eq!(a.todos[0].notes,"edited on B");
        a.todos.retain(|todo|todo.id!="one");da.observe(&a,"A").unwrap();assert!(cas(revision,&da));
        b.todos[0].title="stale offline edit".into();db.observe(&b,"B").unwrap();let (_,remote)=fetch();db.merge(&remote).unwrap();db.apply(&mut b).unwrap();assert!(!b.todos.iter().any(|todo|todo.id=="one"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn eight_devices_join_collide_and_converge() {
        use crate::sync_document::{Document,decode};
        let root=std::env::temp_dir().join(format!("desk-todo-many-devices-{}",uuid::Uuid::new_v4()));
        let config=settings(root.to_string_lossy().into_owned());
        let key=crate::server_crypto::ServerKey::for_test();
        let call=|mode:&str,input:&[u8]| {let mut command=Command::new("sh");command.arg("-c").arg(database_command(&config,mode,Some(key.id)).unwrap());execute(command,input).unwrap()};
        let fetch=|| {let bytes=call("fetch",&[]);(u64::from_le_bytes(bytes[..8].try_into().unwrap()),if bytes.len()>8 {decode(&key.decrypt(&bytes[8..]).unwrap()).unwrap()} else {Document {schema:1,..Default::default()}})};
        let cas=|revision:u64,document:&Document| {let mut bytes=revision.to_le_bytes().to_vec();bytes.extend(key.encrypt(&serde_json::to_vec(document).unwrap()).unwrap());call("cas",&bytes)==[1]};
        let mut seed=AppData::default();
        seed.todos.push(serde_json::from_str(r#"{"id":"shared","title":"base","notes":"old","order":0}"#).unwrap());
        let (_,empty)=fetch();let base=Document::bootstrap(&seed,&empty,"seed").unwrap();assert!(cas(0,&base));
        let (stale_revision,remote)=fetch();
        let mut peers=Vec::new();
        for n in 0..8 {
            let device=format!("device-{n}");
            let mut data=AppData::default();let mut document=Document::bootstrap(&data,&remote,&device).unwrap();document.apply(&mut data).unwrap();
            assert_eq!(data.todos.len(),1);
            data.todos.push(serde_json::from_value(serde_json::json!({"id":format!("task-{n}"),"title":format!("from {n}"),"order":n+1})).unwrap());
            data.todos[0].title=format!("shared edit {n}");
            if n==0 {data.todos.remove(0);}
            document.observe(&data,&device).unwrap();peers.push(document);
        }
        for (n,document) in peers.iter_mut().enumerate() {
            let written=cas(stale_revision,document);
            assert_eq!(written,n==0); // All eight writers started from the same revision.
            if !written {let (revision,latest)=fetch();document.merge(&latest).unwrap();assert!(cas(revision,document));}
        }
        let (_,latest)=fetch();
        for document in &mut peers {
            document.merge(&latest).unwrap();
            assert_eq!(serde_json::to_value(&*document).unwrap(),serde_json::to_value(&latest).unwrap());
            let mut data=AppData::default();document.apply(&mut data).unwrap();
            assert_eq!(data.todos.len(),8);assert!(!data.todos.iter().any(|todo|todo.id=="shared"));
            for n in 0..8 {assert!(data.todos.iter().any(|todo|todo.id==format!("task-{n}")));}
        }
        // A ninth device joins after convergence, without deleting existing tasks.
        let mut newcomer=AppData::default();let joined=Document::bootstrap(&newcomer,&latest,"device-8").unwrap();joined.apply(&mut newcomer).unwrap();assert_eq!(newcomer.todos.len(),8);
        std::fs::remove_dir_all(root).unwrap();
    }

}
