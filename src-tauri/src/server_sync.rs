use crate::model::{AppData, ServerSettings};
use crate::{server_auth, server_crypto::{ServerKey,Pairing}, ssh_transport, AppState};
use tauri::{AppHandle, Emitter, Manager};
use crate::sync_document::{self,Document,SyncState};
use serde::Serialize;

pub fn upload(app: &AppHandle) -> Result<String, String> {
    let state = app.state::<AppState>();
    let _busy = state
        .server_busy
        .try_lock()
        .map_err(|_| "服务器上传正在进行中".to_string())?;
    let snapshot = {
        let mut current=state.data.lock().unwrap();
        sync_document::track(&mut current,&state.dir)?;
        current.clone()
    };
    let mut settings = snapshot.settings.server.clone();
    settings.known_hosts_path = state.dir.join("server-known-hosts");
    let key=ServerKey::load(&state.dir,true)?;
    let group=key.id.to_string();
    let device=sync_document::device_id(&state.dir)?;
    let mut local=snapshot.server_sync.as_ref().filter(|state|state.group_id==group).map(|state|state.document.clone());
    let mut accepted=None;
    for _ in 0..5 {
        let (revision,ciphertext)=ssh_transport::fetch(&settings,key.id)?;
        let remote=match ciphertext {Some(bytes)=>sync_document::decode(&key.decrypt(&bytes)?)?,None=>Document {schema:1,..Default::default()}};
        let document=match &mut local {
            Some(document)=>{document.merge(&remote)?;document.clone()},
            None=>{let document=Document::bootstrap(&snapshot,&remote,&device)?;local=Some(document.clone());document},
        };
        if revision>0 && serde_json::to_value(&document).map_err(|_|"同步数据序列化失败")?==serde_json::to_value(&remote).map_err(|_|"同步数据序列化失败")? {
            accepted=Some(document);break;
        }
        let plaintext=zeroize::Zeroizing::new(serde_json::to_vec(&document).map_err(|_|"同步数据序列化失败")?);
        let encrypted=key.encrypt(&plaintext)?;
        if ssh_transport::compare_and_swap(&settings,revision,&encrypted)? {accepted=Some(document);break;}
    }
    let mut accepted=accepted.ok_or("其他设备正在频繁修改，请稍后重试；本机改动已保留")?;
    {
        let mut current=state.data.lock().unwrap();
        let now=&current.settings.server;
        if now.host!=settings.host || now.port!=settings.port || now.username!=settings.username
            || now.remote_dir!=settings.remote_dir || now.database_name!=settings.database_name {
            return Err("同步期间服务器配置改变，本机改动保留，请重新同步".into());
        }
        let mut pending=match &snapshot.server_sync {
            Some(state) if state.group_id==group=>state.document.clone(),
            _=>{let mut baseline=Document::default();baseline.observe(&snapshot,&device)?;baseline},
        };
        // 网络操作期间的修改再观察一次，不能被下载结果覆盖。
        pending.clock=pending.clock.max(accepted.clock);
        pending.observe(&current,&device)?;
        accepted.merge(&pending)?;
        let mut updated=current.clone();
        accepted.apply(&mut updated)?;
        updated.server_sync=Some(SyncState {group_id:group,document:accepted});
        updated.settings.server.last_upload=Some(crate::model::now_rfc3339());
        crate::store::save(&state.dir,&updated)?;
        *current=updated;
    }
    let _=crate::tray::refresh(app);
    let _=app.emit("server://synced",());
    let message="跨电脑同步完成，已合并服务器与本机待办".to_string();
    let _ = app.emit("server://uploaded", &message);
    Ok(message)
}

#[tauri::command]
pub async fn server_upload(app: AppHandle) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || upload(&app))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn server_test_connection(app: AppHandle, mut server: ServerSettings) -> Result<String, String> {
    server.known_hosts_path = app.state::<AppState>().dir.join("server-known-hosts");
    tauri::async_runtime::spawn_blocking(move || ssh_transport::test_connection(&server))
        .await
        .map_err(|e| e.to_string())?
}

/// 解密只在桌面端进行；解密成功前不修改本机数据。
#[tauri::command]
pub async fn server_read_backup(app: AppHandle) -> Result<AppData, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _busy = state.server_busy.try_lock().map_err(|_| "服务器操作正在进行中".to_string())?;
        let mut settings = state.data.lock().unwrap().settings.server.clone();
        settings.known_hosts_path = state.dir.join("server-known-hosts");
        let key = ServerKey::load(&state.dir, false)?;
        let encrypted = ssh_transport::download(&settings, key.id)?;
        let plaintext = key.decrypt(&encrypted)?;
        let document=sync_document::decode(&plaintext)?;
        let mut backup=AppData::default();document.apply(&mut backup)?;Ok(backup)
    }).await.map_err(|e| e.to_string())?
}

/// 前端展示具体条目数量并确认后应用已读取的备份，保留本机设置。
#[tauri::command]
pub fn server_restore_backup(app: AppHandle, mut backup: AppData) -> Result<AppData, String> {
    let state = app.state::<AppState>();
    crate::model::normalize(&mut backup.todos);
    let data = {
        let mut current = state.data.lock().unwrap();
        let mut restored = current.clone();
        restored.todos = backup.todos;
        restored.trash = backup.trash;
        restored.last_sync = backup.last_sync;
        sync_document::track(&mut restored,&state.dir)?;
        crate::store::save(&state.dir, &restored)?;
        *current = restored.clone();
        restored
    };
    if let Ok(mut dirty) = state.git.dirty.lock() {
        *dirty = true;
    }
    let _ = crate::tray::refresh(&app);
    Ok(data)
}

#[tauri::command]
pub async fn server_save_password(server: ServerSettings, password: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || server_auth::save_password(&server, password))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn server_scan_host(server: ServerSettings) -> Result<server_auth::HostPreview, String> {
    tauri::async_runtime::spawn_blocking(move || server_auth::scan_host(&server))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn server_trust_host(app: AppHandle, confirmation_id: String) -> Result<String, String> {
    let dir = app.state::<AppState>().dir.clone();
    tauri::async_runtime::spawn_blocking(move || server_auth::trust_host(&dir, &confirmation_id))
        .await.map_err(|e| e.to_string())?
}

#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct PairPreview {pub host:String,pub port:u16,pub group_id:String}
#[tauri::command]
pub async fn server_create_pairing(app:AppHandle)->Result<String,String> {
    tauri::async_runtime::spawn_blocking(move|| {
        let state=app.state::<AppState>();
        let _busy=state.server_busy.try_lock().map_err(|_|"服务器操作正在进行中")?;
        let server=state.data.lock().unwrap().settings.server.clone();
        server_auth::password_account(&server)?;
        let key=ServerKey::load(&state.dir,true)?;
        let host_key=server_auth::paired_host_key(&state.dir,&server);
        key.pairing_code(server,host_key)
    }).await.map_err(|e|e.to_string())?
}
#[tauri::command]
pub fn server_preview_pairing(code:String)->Result<PairPreview,String> {
    let pairing=Pairing::parse(&code)?;
    Ok(PairPreview {host:pairing.server.host.clone(),port:pairing.server.port,group_id:pairing.id.to_string()})
}
#[tauri::command]
pub async fn server_join_pairing(app:AppHandle,code:String)->Result<AppData,String> {
    tauri::async_runtime::spawn_blocking(move|| {
        let pairing=Pairing::parse(&code)?;
        let state=app.state::<AppState>();
        let _busy=state.server_busy.try_lock().map_err(|_|"服务器操作正在进行中")?;
        let mut current=state.data.lock().unwrap();
        let old_id=std::fs::read(state.dir.join(".server-key-id")).ok();
        let rollback=|| {
            if let Some(id)=&old_id {let _=crate::model::atomic_write(&state.dir.join(".server-key-id"),id);}
            else {let _=std::fs::remove_file(state.dir.join(".server-key-id"));}
        };
        pairing.install(&state.dir)?;
        if let Some(host_key)=&pairing.host_key {
            if let Err(error)=server_auth::trust_paired_host(&state.dir,&pairing.server,host_key) {
                rollback();
                return Err(error);
            }
        }
        let mut joined=current.clone();
        joined.settings.sync_method=crate::model::SyncMethod::Ssh;
        joined.settings.server=pairing.server.clone();
        joined.settings.server.auto_upload=true;
        joined.settings.server.interval_minutes=1;
        joined.server_sync=None;
        if let Err(error)=crate::store::save(&state.dir,&joined) {
            rollback();
            return Err(error);
        }
        *current=joined.clone();
        Ok(joined)
    }).await.map_err(|e|e.to_string())?
}
