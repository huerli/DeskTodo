//! 数据持久化：原子写入 + 版本化备份 + 损坏自愈

use crate::model::{self, AppData};
use std::path::{Path, PathBuf};

/// 备份间隔：距上次备份超过该秒数才生成新备份，避免频繁写入放大
const BACKUP_INTERVAL_SECS: i64 = 3600;
const MAX_BACKUPS: usize = 12;

pub fn load(dir: &Path) -> AppData {
    let file = model::data_file(dir);
    match std::fs::read_to_string(&file) {
        Ok(text) => match serde_json::from_str::<AppData>(&text) {
            Ok(mut data) => {
                if data.version == 0 {
                    data.version = model::DATA_VERSION;
                }
                model::normalize(&mut data.todos);
                data
            }
            Err(e) => {
                // 数据损坏：留档原文，尝试最近的备份，最后退回空数据
                eprintln!("[store] todos.json 解析失败: {e}");
                let broken = dir.join(format!(
                    "todos.corrupted-{}.json",
                    chrono::Local::now().format("%Y%m%d-%H%M%S")
                ));
                let _ = std::fs::rename(&file, &broken);
                if let Some(data) = load_latest_backup(dir) {
                    return data;
                }
                AppData::default()
            }
        },
        Err(_) => {
            // 首次运行
            if let Some(data) = load_latest_backup(dir) {
                return data;
            }
            AppData::default()
        }
    }
}

fn load_latest_backup(dir: &Path) -> Option<AppData> {
    let mut items: Vec<PathBuf> = std::fs::read_dir(model::backup_dir(dir))
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .map(|e| e == "json")
                .unwrap_or(false)
        })
        .collect();
    // 文件名以时间戳开头，字典序即时间序
    items.sort();
    for p in items.iter().rev() {
        if let Ok(text) = std::fs::read_to_string(p) {
            if let Ok(mut data) = serde_json::from_str::<AppData>(&text) {
                model::normalize(&mut data.todos);
                eprintln!("[store] 已从备份恢复: {}", p.display());
                return Some(data);
            }
        }
    }
    None
}

pub fn save(dir: &Path, data: &AppData) -> Result<(), String> {
    let file = model::data_file(dir);
    maybe_backup(dir, &file);
    let json = serde_json::to_vec_pretty(data).map_err(|e| format!("序列化失败: {e}"))?;
    model::atomic_write(&file, &json)
}

fn maybe_backup(dir: &Path, file: &Path) {
    if !file.exists() {
        return;
    }
    let bdir = model::backup_dir(dir);
    if std::fs::create_dir_all(&bdir).is_err() {
        return;
    }

    // 距最近一次备份不足间隔则跳过
    if let Ok(entries) = std::fs::read_dir(&bdir) {
        let mut stamps: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.ends_with(".json"))
            .collect();
        stamps.sort();
        if let Some(last) = stamps.last() {
            let name = last.trim_end_matches(".json");
            if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(name, "%Y%m%d-%H%M%S") {
                let age = chrono::Local::now().naive_local() - dt;
                if age.num_seconds() < BACKUP_INTERVAL_SECS {
                    return;
                }
            }
        }
    }

    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let _ = std::fs::copy(file, bdir.join(format!("{stamp}.json")));
    prune(&bdir);
}

fn prune(bdir: &Path) {
    let Ok(entries) = std::fs::read_dir(bdir) else {
        return;
    };
    let mut items: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|e| e == "json").unwrap_or(false))
        .collect();
    items.sort();
    if items.len() > MAX_BACKUPS {
        for p in &items[..items.len() - MAX_BACKUPS] {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// 导出为单个 JSON 文件（用于备份/换机）
pub fn export_to(dir: &Path, data: &AppData, target: &Path) -> Result<(), String> {
    let _ = dir;
    let json = serde_json::to_vec_pretty(data).map_err(|e| format!("序列化失败: {e}"))?;
    std::fs::write(target, json).map_err(|e| format!("写入导出文件失败: {e}"))
}

/// 从导出的 JSON 读取数据
pub fn import_from(path: &Path) -> Result<AppData, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("读取失败: {e}"))?;
    let mut data: AppData =
        serde_json::from_str(&text).map_err(|e| format!("文件格式不正确: {e}"))?;
    model::normalize(&mut data.todos);
    // 导入的窗口/同步设置不覆盖本机配置
    Ok(data)
}
