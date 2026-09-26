//! 每日工作情况总结
//!
//! 把「当天完成」的待办汇总成一份 Markdown 日报，写入数据目录的 `reports/`。
//! 每次有待办完成时即时重算 —— 当天无论完成几次，文件都会被更新成最新版本，
//! 不需要等到第二天，也不需要额外的定时任务。

use crate::model::{AppData, DailySummarySettings, Todo};
use std::path::{Path, PathBuf};

pub const REPORT_DIR: &str = "reports";

fn report_file_name(settings: &DailySummarySettings, date: &str) -> String {
    let tpl = settings.filename.trim();
    let tpl = if tpl.is_empty() { "{date}.md" } else { tpl };
    let name = tpl.replace("{date}", date);
    // 防止模板里带路径分隔符导致写到别处
    name.replace(['/', '\\'], "_")
}

fn parse_local(ts: &str) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|d| d.with_timezone(&chrono::Local))
}

fn priority_mark(p: crate::model::Priority) -> &'static str {
    match p {
        crate::model::Priority::High => " **[高优]**",
        crate::model::Priority::Low => " [低]",
        crate::model::Priority::Normal => "",
    }
}

/// 生成指定日期的总结文本。`date` 形如 `2026-09-26`（本地时区）。
pub fn render(data: &AppData, date: &str) -> String {
    let settings = &data.settings.daily_summary;
    let mut done: Vec<&Todo> = data
        .todos
        .iter()
        .filter(|t| t.done)
        .filter(|t| {
            t.completed_at
                .as_deref()
                .and_then(parse_local)
                .map(|d| d.format("%Y-%m-%d").to_string() == date)
                .unwrap_or(false)
        })
        .collect();
    // 按完成时间排序，让总结顺序与实际推进顺序一致
    done.sort_by_key(|t| t.completed_at.clone().unwrap_or_default());

    let mut out = String::new();
    out.push_str(&format!("# {date} 工作情况\n\n"));

    if done.is_empty() {
        out.push_str("今日暂无已完成的待办事项。\n");
        return out;
    }

    // 概览
    let high = done
        .iter()
        .filter(|t| t.priority == crate::model::Priority::High)
        .count();
    let mut overview = format!("**完成 {} 项**", done.len());
    if high > 0 {
        overview.push_str(&format!(" · 其中高优 {high} 项"));
    }
    let first = done
        .first()
        .and_then(|t| t.completed_at.as_deref())
        .and_then(parse_local)
        .map(|d| d.format("%H:%M").to_string());
    let last = done
        .last()
        .and_then(|t| t.completed_at.as_deref())
        .and_then(parse_local)
        .map(|d| d.format("%H:%M").to_string());
    if let (Some(f), Some(l)) = (first, last) {
        if f == l {
            overview.push_str(&format!(" · {f}"));
        } else {
            overview.push_str(&format!(" · {f}–{l}"));
        }
    }
    out.push_str(&overview);
    out.push_str("\n\n## 完成明细\n\n");

    for (i, t) in done.iter().enumerate() {
        let time = t
            .completed_at
            .as_deref()
            .and_then(parse_local)
            .map(|d| d.format("%H:%M").to_string())
            .unwrap_or_else(|| "--:--".into());
        // 标题可能含换行，压成一行保证 Markdown 列表结构正确
        let title = t.title.replace('\n', " ").trim().to_string();
        out.push_str(&format!(
            "{}. `{time}` {}{}\n",
            i + 1,
            title,
            priority_mark(t.priority)
        ));
        if settings.include_notes {
            let notes = t.notes.trim();
            if !notes.is_empty() {
                for line in notes.lines() {
                    // 缩进到列表项下，保持层级
                    out.push_str(&format!("   > {}\n", line.trim()));
                }
            }
        }
        if !t.tags.is_empty() {
            let tags = t
                .tags
                .iter()
                .map(|s| format!("#{s}"))
                .collect::<Vec<_>>()
                .join(" ");
            out.push_str(&format!("   标签：{tags}\n"));
        }
    }

    // 未完成事项也列出来，便于次日接力
    let pending: Vec<&Todo> = data.todos.iter().filter(|t| !t.done).collect();
    if !pending.is_empty() {
        out.push_str("\n## 待跟进\n\n");
        for t in pending {
            let title = t.title.replace('\n', " ").trim().to_string();
            let due = t
                .due_at
                .as_deref()
                .and_then(parse_local)
                .map(|d| format!("（{}）", d.format("%m-%d %H:%M")))
                .unwrap_or_default();
            out.push_str(&format!("- [ ] {}{}{}\n", title, priority_mark(t.priority), due));
        }
    }

    out.push_str(&format!(
        "\n---\n\n<sub>由 DeskTodo 于 {} 自动生成</sub>\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M")
    ));
    out
}

/// 写入数据目录下的 reports/<name>。返回写入的路径。
pub fn write_report(dir: &Path, name: &str, body: &str) -> Result<PathBuf, String> {
    let rdir = dir.join(REPORT_DIR);
    std::fs::create_dir_all(&rdir).map_err(|e| format!("创建 reports 目录失败: {e}"))?;
    let path = rdir.join(name);
    std::fs::write(&path, body).map_err(|e| format!("写入总结失败: {e}"))?;
    Ok(path)
}

/// 生成「指定日期」的总结并落盘；同时按配置额外导出一份。
/// 返回 (数据目录内的路径, 导出路径)
pub fn generate_for(
    dir: &Path,
    data: &AppData,
    date: &str,
) -> Result<(PathBuf, Option<PathBuf>), String> {
    let settings = &data.settings.daily_summary;
    let name = report_file_name(settings, date);
    let body = render(data, date);
    let path = write_report(dir, &name, &body)?;

    let export = settings.export_dir.trim();
    if export.is_empty() {
        return Ok((path, None));
    }
    // 支持 ~ 开头的路径
    let expanded = if let Some(rest) = export.strip_prefix("~/") {
        dirs::home_dir()
            .map(|h| h.join(rest))
            .unwrap_or_else(|| PathBuf::from(export))
    } else {
        PathBuf::from(export)
    };
    std::fs::create_dir_all(&expanded)
        .map_err(|e| format!("创建导出目录 {} 失败: {e}", expanded.display()))?;
    let out = expanded.join(&name);
    std::fs::write(&out, &body).map_err(|e| format!("导出总结失败: {e}"))?;
    Ok((path, Some(out)))
}

/// 生成今天的总结
pub fn generate_today(dir: &Path, data: &AppData) -> Result<(PathBuf, Option<PathBuf>), String> {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    generate_for(dir, data, &today)
}

/// 今天完成了多少项
pub fn today_done_count(data: &AppData) -> usize {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    data.todos
        .iter()
        .filter(|t| t.done)
        .filter(|t| {
            t.completed_at
                .as_deref()
                .and_then(parse_local)
                .map(|d| d.format("%Y-%m-%d").to_string() == today)
                .unwrap_or(false)
        })
        .count()
}
