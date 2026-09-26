//! 数据模型 + 磁盘读写

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const DATA_VERSION: u32 = 1;

fn default_true() -> bool {
    true
}
fn default_lead() -> u32 {
    0
}
fn default_interval() -> u32 {
    60
}
fn default_opacity() -> f64 {
    0.96
}
fn default_theme() -> String {
    "auto".into()
}
fn default_remote_name() -> String {
    "origin".into()
}
fn default_branch() -> String {
    "main".into()
}
fn default_commit_message() -> String {
    "desk-todo: 同步待办数据".into()
}
fn default_sort() -> SortMode {
    SortMode::Manual
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Low,
    Normal,
    High,
}

impl Default for Priority {
    fn default() -> Self {
        Priority::Normal
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortMode {
    Manual,
    Due,
    Created,
    Priority,
}

impl Default for SortMode {
    fn default() -> Self {
        SortMode::Manual
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Todo {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub done: bool,
    /// RFC3339 本地时间字符串
    #[serde(default)]
    pub due_at: Option<String>,
    #[serde(default)]
    pub priority: Priority,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub completed_at: Option<String>,
    /// 已提醒过的时间戳，避免重复提醒
    #[serde(default)]
    pub notified_at: Option<String>,
    /// 手动排序序号
    #[serde(default)]
    pub order: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowSettings {
    #[serde(default)]
    pub position: Option<(i32, i32)>,
    /// 展开状态下的窗口尺寸（收起时不会被覆盖）
    #[serde(default)]
    pub size: Option<(f64, f64)>,
    #[serde(default)]
    pub collapsed: bool,
    #[serde(default = "default_true")]
    pub always_on_top: bool,
    #[serde(default = "default_opacity")]
    pub opacity: f64,
    #[serde(default = "default_theme")]
    pub theme: String,
}

/// 收起时的高度（逻辑像素）。
/// 必须容纳 titlebar(38px) + 收起摘要条(26px) 加少量留白，
/// 否则摘要行会被窗口裁掉一半（曾用 52px，导致下半截内容露出）。
pub const COLLAPSED_HEIGHT: f64 = 68.0;
/// 小于该高度的尺寸不写入记忆值，避免把收起高度当成展开高度
pub const MIN_EXPANDED_HEIGHT: f64 = 120.0;

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            position: None,
            size: None,
            collapsed: false,
            always_on_top: true,
            opacity: default_opacity(),
            theme: default_theme(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReminderSettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 提前多少分钟提醒
    #[serde(default = "default_lead")]
    pub lead_minutes: u32,
    #[serde(default = "default_true")]
    pub sound: bool,
    #[serde(default)]
    pub voice: bool,
    #[serde(default = "default_true")]
    pub notify_overdue: bool,
    #[serde(default)]
    pub daily_digest: bool,
}

impl Default for ReminderSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            lead_minutes: 0,
            sound: true,
            voice: false,
            notify_overdue: true,
            daily_digest: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncSettings {
    #[serde(default)]
    pub remote_url: String,
    #[serde(default = "default_remote_name")]
    pub remote_name: String,
    #[serde(default = "default_branch")]
    pub branch: String,
    /// 仅支持 https 远程；建议使用访问令牌
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub ssh_key_path: String,
    #[serde(default)]
    pub author_name: String,
    #[serde(default)]
    pub author_email: String,
    #[serde(default = "default_true")]
    pub auto_commit: bool,
    #[serde(default = "default_interval")]
    pub auto_sync_minutes: u32,
    #[serde(default = "default_commit_message")]
    pub commit_message: String,
}

impl Default for SyncSettings {
    fn default() -> Self {
        Self {
            remote_url: String::new(),
            remote_name: default_remote_name(),
            branch: default_branch(),
            username: String::new(),
            token: String::new(),
            ssh_key_path: String::new(),
            author_name: String::new(),
            author_email: String::new(),
            auto_commit: true,
            auto_sync_minutes: default_interval(),
            commit_message: default_commit_message(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    #[serde(default)]
    pub window: WindowSettings,
    #[serde(default)]
    pub reminder: ReminderSettings,
    #[serde(default)]
    pub sync: SyncSettings,
    #[serde(default = "default_sort")]
    pub sort: SortMode,
    /// 启动时自动隐藏到托盘
    #[serde(default)]
    pub start_hidden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppData {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub todos: Vec<Todo>,
    #[serde(default)]
    pub settings: Settings,
    /// 最近删除，用于撤销
    #[serde(default)]
    pub trash: Vec<Todo>,
    #[serde(default)]
    pub notices: HashMap<String, String>,
    #[serde(default)]
    pub last_sync: Option<String>,
    /// 数据目录（仅返回给前端展示，不从磁盘读取）
    #[serde(default, skip_deserializing)]
    pub data_dir: String,
}

fn default_version() -> u32 {
    DATA_VERSION
}

impl Default for AppData {
    fn default() -> Self {
        Self {
            version: DATA_VERSION,
            todos: Vec::new(),
            settings: Settings::default(),
            trash: Vec::new(),
            notices: HashMap::new(),
            last_sync: None,
            data_dir: String::new(),
        }
    }
}

pub fn data_file(dir: &Path) -> PathBuf {
    dir.join("todos.json")
}

pub fn backup_dir(dir: &Path) -> PathBuf {
    dir.join("backups")
}

/// 原子写入：先写临时文件再 rename，避免断电/崩溃导致文件损坏
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("写入临时文件失败: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("替换数据文件失败: {e}"))?;
    Ok(())
}

pub fn now_rfc3339() -> String {
    chrono::Local::now().to_rfc3339()
}

/// 规范化待办数据：修正 order、补全时间戳
pub fn normalize(todos: &mut [Todo]) {
    for (i, t) in todos.iter_mut().enumerate() {
        if t.id.trim().is_empty() {
            t.id = uuid::Uuid::new_v4().to_string();
        }
        t.title = t.title.trim().to_string();
        if t.order == 0 {
            t.order = (i as i64 + 1) * 10;
        }
        if t.created_at.is_empty() {
            t.created_at = now_rfc3339();
        }
        if t.updated_at.is_empty() {
            t.updated_at = t.created_at.clone();
        }
    }
    todos.sort_by_key(|t| t.order);
}
