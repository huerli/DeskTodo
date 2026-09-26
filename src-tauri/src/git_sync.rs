//! Git 同步
//!
//! 分层策略：
//! - 本地操作（init / status / add / commit / log）走 `git2`（libgit2），纯本地、无网络依赖；
//! - 网络操作（fetch / pull / push）走系统 `git` 命令，这样能直接复用用户已经配置好的
//!   SSH key、ssh-agent、credential helper（macOS Keychain / Windows 凭据管理器），
//!   避免把令牌写进仓库配置，也能规避各平台 TLS 差异。

use crate::model::SyncSettings;
use crate::AppState;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    pub is_repo: bool,
    pub configured: bool,
    pub git_available: bool,
    pub branch: String,
    pub remote_url: String,
    pub dirty: bool,
    pub changed_files: Vec<String>,
    pub last_commit: Option<String>,
    pub last_commit_at: Option<String>,
    pub last_sync: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitResult {
    pub ok: bool,
    pub message: String,
    pub committed: bool,
    pub pushed: bool,
    pub pulled: bool,
    pub status: GitStatus,
}

pub struct GitManager {
    pub dir: PathBuf,
    pub dirty: Mutex<bool>,
    pub busy: Mutex<bool>,
    pub last_error: Mutex<Option<String>>,
}

impl GitManager {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            dirty: Mutex::new(false),
            busy: Mutex::new(false),
            last_error: Mutex::new(None),
        }
    }

    pub fn repo(&self) -> Result<git2::Repository, String> {
        git2::Repository::open(&self.dir).map_err(|e| format!("打开仓库失败: {e}"))
    }

    /// 暂存全部数据文件并提交（无变更则返回 false）
    pub fn commit_all(&self, settings: &SyncSettings, message: &str) -> Result<bool, String> {
        let repo = self.repo()?;
        stage_all(&repo)?;

        let mut index = repo.index().map_err(|e| e.to_string())?;
        let tree_oid = index.write_tree().map_err(|e| e.to_string())?;
        let tree = repo.find_tree(tree_oid).map_err(|e| e.to_string())?;

        let parent = match repo.head().ok().and_then(|h| h.peel_to_commit().ok()) {
            Some(c) => Some(c),
            None => None,
        };
        if let Some(p) = &parent {
            if p.tree_id() == tree_oid {
                return Ok(false); // 无变化
            }
        }

        let sig = signature(&repo, settings)?;
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
            .map_err(|e| format!("提交失败: {e}"))?;
        Ok(true)
    }

    /// 完整同步：本地提交 → fetch → 变基 → 推送
    pub fn sync_now(
        &self,
        message: &str,
        settings_override: Option<SyncSettings>,
        app: Option<&AppHandle>,
    ) -> Result<GitResult, String> {
        let settings = match settings_override {
            Some(s) => s,
            None => app
                .map(|a| {
                    let st = a.state::<AppState>();
                    let d = st.data.lock().unwrap();
                    d.settings.sync.clone()
                })
                .unwrap_or_default(),
        };

        if let Ok(mut b) = self.busy.lock() {
            if *b {
                return Err("同步正在进行中".into());
            }
            *b = true;
        }
        let result = self.sync_inner(&settings, message);
        if let Ok(mut b) = self.busy.lock() {
            *b = false;
        }
        match &result {
            Ok(_) => {
                if let Ok(mut e) = self.last_error.lock() {
                    *e = None;
                }
                if let Ok(mut d) = self.dirty.lock() {
                    *d = false;
                }
            }
            Err(err) => {
                if let Ok(mut e) = self.last_error.lock() {
                    *e = Some(err.clone());
                }
            }
        }
        result
    }

    fn sync_inner(&self, settings: &SyncSettings, message: &str) -> Result<GitResult, String> {
        let mut result = GitResult {
            ok: false,
            message: String::new(),
            committed: false,
            pushed: false,
            pulled: false,
            status: GitStatus::default(),
        };

        if git_binary().is_none() {
            return Err("未找到系统 git，请先安装 Git 后再使用同步功能".into());
        }
        if !self.dir.join(".git").is_dir() {
            init_repo(&self.dir, settings)?;
        }
        // 提交本地改动前先确保有东西可提交
        result.committed = self.commit_all(settings, message).unwrap_or(false);

        let remote = settings.remote_name.trim();
        let remote = if remote.is_empty() { "origin" } else { remote };

        // 未配置远程仓库：只做本地提交
        if settings.remote_url.trim().is_empty() && run_git(&self.dir, settings, &["remote", "get-url", remote]).is_err() {
            result.ok = true;
            result.message = if result.committed {
                "已提交到本地仓库（尚未配置远程仓库）".into()
            } else {
                "没有需要同步的改动".into()
            };
            result.status = self.status(settings);
            return Ok(result);
        }

        ensure_remote(&self.dir, settings, remote)?;

        // fetch + 变基 + 推送
        let mut notes: Vec<String> = Vec::new();
        match run_git(&self.dir, settings, &["fetch", "--prune", remote]) {
            Ok(_) => notes.push("已获取远程更新".into()),
            Err(e) => return Err(format!("拉取远端失败：{e}")),
        }

        let upstream = format!("{remote}/{}", settings.branch);
        if run_git(&self.dir, settings, &["rev-parse", "--verify", &upstream]).is_ok() {
            if let Ok(local_head) = run_git(&self.dir, settings, &["rev-parse", "HEAD"]) {
                let local_head = local_head.trim().to_string();
                let remote_head = run_git(&self.dir, settings, &["rev-parse", &upstream])
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                let base = run_git(&self.dir, settings, &["merge-base", "HEAD", &upstream])
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                if local_head != remote_head {
                    let rebase = run_git(
                        &self.dir,
                        settings,
                        &["rebase", "--autostash", &upstream],
                    );
                    match rebase {
                        Ok(_) => {
                            result.pulled = true;
                            notes.push("已合并远程改动".into());
                        }
                        Err(e) => {
                            let _ = run_git(&self.dir, settings, &["rebase", "--abort"]);
                            let hint = if base.is_empty() {
                                "本地与远程历史无关"
                            } else {
                                "存在冲突"
                            };
                            return Err(format!(
                                "{hint}，自动同步已中止（本地数据未受影响）。请手动处理后重试。\n{e}"
                            ));
                        }
                    }
                }
            }
        }

        let head_branch =
            current_branch(&self.dir).unwrap_or_else(|| settings.branch.clone());
        let refspec = format!("HEAD:refs/heads/{head_branch}");
        match run_git(&self.dir, settings, &["push", "-u", remote, &refspec]) {
            Ok(_) => {
                result.pushed = true;
                notes.push("已推送到远程".into());
            }
            Err(e) => {
                if settings.token.trim().is_empty() {
                    return Err(format!(
                        "推送失败：{e}\n提示：HTTPS 私有仓库需要在设置里填写用户名与访问令牌，或改用 SSH 地址。"
                    ));
                }
                return Err(format!("推送失败：{e}"));
            }
        }

        result.ok = true;
        result.message = if notes.is_empty() {
            "已是最新".into()
        } else {
            notes.join("；")
        };
        result.status = self.status(settings);
        Ok(result)
    }

    pub fn status(&self, settings: &SyncSettings) -> GitStatus {
        let mut s = GitStatus {
            branch: settings.branch.clone(),
            remote_url: settings.remote_url.clone(),
            configured: !settings.remote_url.trim().is_empty(),
            git_available: git_binary().is_some(),
            ..Default::default()
        };

        let Ok(repo) = self.repo() else {
            s.message = "尚未初始化本地仓库".into();
            return s;
        };
        s.is_repo = true;

        if let Ok(head) = repo.head() {
            if let Ok(name) = head.shorthand() {
                s.branch = name.to_string();
            }
            if let Ok(commit) = head.peel_to_commit() {
                let short = commit.id().to_string()[..8].to_string();
                let summary_text = match commit.summary() {
                    Ok(Some(s)) => s.to_string(),
                    _ => short,
                };
                s.last_commit = Some(summary_text);
                let secs = commit.time().seconds();
                s.last_commit_at = chrono::DateTime::from_timestamp(secs, 0)
                    .map(|dt| dt.with_timezone(&chrono::Local).to_rfc3339());
            }
        }

        // 工作区是否有改动
        if let Ok(diffs) = repo.statuses(Some(
            git2::StatusOptions::new()
                .include_untracked(true)
                .recurse_untracked_dirs(true),
        )) {
            for entry in diffs.iter() {
                if let Ok(p) = entry.path() {
                    s.changed_files.push(p.to_string());
                }
            }
        }
        s.dirty = !s.changed_files.is_empty();

        // 与上游的领先/落后数量（需要上游引用已存在）
        let remote = if settings.remote_name.trim().is_empty() {
            "origin"
        } else {
            settings.remote_name.trim()
        };
        let upstream = format!("{remote}/{}", s.branch);
        if let (Ok(local), Ok(up)) = (
            repo.revparse_single("HEAD").and_then(|o| o.peel_to_commit()),
            repo.revparse_single(&upstream).and_then(|o| o.peel_to_commit()),
        ) {
            if let Ok((a, b)) = repo.graph_ahead_behind(local.id(), up.id()) {
                s.ahead = a;
                s.behind = b;
            }
        }

        s.message = if s.dirty {
            format!("{} 个文件待提交", s.changed_files.len())
        } else if s.ahead > 0 {
            format!("{} 个提交待推送", s.ahead)
        } else if s.behind > 0 {
            format!("远端领先 {} 个提交", s.behind)
        } else {
            "已同步".into()
        };
        s
    }
}

// ------------------------------------------------------------ 底层辅助

fn signature(repo: &git2::Repository, settings: &SyncSettings) -> Result<git2::Signature<'static>, String> {
    let name = if settings.author_name.trim().is_empty() {
        "DeskTodo"
    } else {
        settings.author_name.trim()
    };
    let email = if settings.author_email.trim().is_empty() {
        "desktodo@localhost"
    } else {
        settings.author_email.trim()
    };
    match git2::Signature::now(name, email) {
        Ok(sig) => Ok(sig),
        Err(_) => repo
            .signature()
            .map(|s| s.to_owned())
            .map_err(|e| format!("无法创建提交签名（请在设置里填写用户名和邮箱）: {e}")),
    }
}

fn stage_all(repo: &git2::Repository) -> Result<(), String> {
    write_gitignore(repo.workdir().unwrap_or(Path::new(".")));
    let mut index = repo.index().map_err(|e| e.to_string())?;
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .map_err(|e| format!("暂存失败: {e}"))?;
    // 移除已被删除文件的索引项
    index
        .update_all(["*"].iter(), None)
        .map_err(|e| format!("更新索引失败: {e}"))?;
    index.write().map_err(|e| e.to_string())?;
    Ok(())
}

/// 需要排除在版本库之外的运行时文件。
/// `boot-trace.log` / `frontend.log` 是诊断日志，进版本库只会污染历史。
const GITIGNORE_PATTERNS: [&str; 5] = [
    "*.tmp",
    "*.log",
    "todos.json.tmp",
    "backups/",
    "todos.corrupted-*.json",
];

/// 幂等地维护 .gitignore：缺失的规则会被追加，已存在的内容不动。
/// 之前只在文件不存在时创建，导致已有仓库拿不到新增的忽略规则。
fn write_gitignore(dir: &Path) {
    let path = dir.join(".gitignore");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();

    let missing: Vec<&str> = GITIGNORE_PATTERNS
        .iter()
        .copied()
        .filter(|p| !existing.lines().any(|l| l.trim() == *p))
        .collect();
    if missing.is_empty() {
        return;
    }

    let mut content = existing;
    if content.is_empty() {
        content.push_str("# 由 DeskTodo 自动生成：运行时数据不入版本库\n");
    } else if !content.ends_with('\n') {
        content.push('\n');
    }
    for p in missing {
        content.push_str(p);
        content.push('\n');
    }
    let _ = std::fs::write(&path, content);
}

pub fn init_repo(dir: &Path, _settings: &SyncSettings) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    write_gitignore(dir);
    let repo = git2::Repository::init(dir).map_err(|e| format!("初始化仓库失败: {e}"))?;
    // 统一使用 main 分支，避免不同平台默认分支不一致
    let _ = repo.set_head("refs/heads/main");
    Ok(())
}

fn ensure_remote(dir: &Path, settings: &SyncSettings, remote: &str) -> Result<(), String> {
    let url = settings.remote_url.trim();
    if url.is_empty() {
        return Err("请先在设置里填写远程仓库地址".into());
    }
    let current = run_git(dir, settings, &["remote", "get-url", remote]);
    match current {
        Ok(existing) if existing.trim() == url => {}
        Ok(_) => {
            run_git(dir, settings, &["remote", "set-url", remote, url])?;
        }
        Err(_) => {
            run_git(dir, settings, &["remote", "add", remote, url])?;
        }
    }
    // 保证本地分支存在
    let branch = settings.branch.trim();
    let branch = if branch.is_empty() { "main" } else { branch };
    if run_git(dir, settings, &["rev-parse", "--verify", "HEAD"]).is_err() {
        let _ = run_git(dir, settings, &["checkout", "-B", branch]);
    }
    Ok(())
}

pub fn git_binary() -> Option<PathBuf> {
    which::which("git").ok()
}

/// 执行系统 git 命令；需要令牌时通过 credential helper 注入，不写入仓库配置
/// 构造 git 使用的 SSH 命令。
///
/// 为什么必须显式设置这些选项：
/// - 应用以 `GIT_TERMINAL_PROMPT=0` 运行（避免卡在交互输入上），
///   此时 SSH 无法向用户询问“是否信任这台主机”，而 OpenSSH 对未知主机的
///   默认策略是 `ask` —— 问不了就直接失败并报
///   `Host key verification failed`。首次使用新主机（如 ssh.github.com）必然踩到。
/// - `accept-new`：首次遇到的主机密钥自动记录，但**密钥变化时仍然拒绝**，
///   安全性等同于手动确认首次连接。
/// - `BatchMode=yes`：绝不进行任何交互，缺凭据时立即失败并返回可读错误。
/// - `IdentitiesOnly=yes`：只使用指定私钥，避免 agent 里过多密钥导致认证失败。
pub fn ssh_command(ssh_key_path: &str) -> String {
    let mut parts = vec![
        "ssh".to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
    ];
    let key = ssh_key_path.trim();
    if !key.is_empty() {
        parts.push("-i".to_string());
        parts.push(key.to_string());
        parts.push("-o".to_string());
        parts.push("IdentitiesOnly=yes".to_string());
    }
    parts.join(" ")
}

/// 执行系统 git 命令；需要令牌时通过 credential helper 注入，不写入仓库配置
fn run_git(dir: &Path, settings: &SyncSettings, args: &[&str]) -> Result<String, String> {
    let git = git_binary().ok_or("未找到系统 git 命令")?;
    let mut cmd = Command::new(git);
    cmd.current_dir(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("LC_ALL", "C")
        // 无条件设置：否则首次连接新主机会报 Host key verification failed
        .env("GIT_SSH_COMMAND", ssh_command(&settings.ssh_key_path));

    let token = settings.token.trim();
    let username = if settings.username.trim().is_empty() {
        "x-access-token"
    } else {
        settings.username.trim()
    };
    if !token.is_empty() {
        // 通过凭据助手动态返回，避免令牌出现在 .git/config 或进程参数里
        let helper = format!(
            "!f() {{ echo username={u}; echo password=${{DESKTODO_GIT_TOKEN}}; }}; f",
            u = username
        );
        cmd.arg("-c").arg(format!("credential.helper={helper}"));
        cmd.env("DESKTODO_GIT_TOKEN", token);
    }

    let out = cmd
        .output()
        .map_err(|e| format!("执行 git {} 失败: {e}", args.join(" ")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() {
            format!("git {} 执行失败", args.join(" "))
        } else {
            err
        })
    }
}

fn current_branch(dir: &Path) -> Option<String> {
    let repo = git2::Repository::open(dir).ok()?;
    let head = repo.head().ok()?;
    head.shorthand().ok().map(|s| s.to_string())
}

// ------------------------------------------------------------ Tauri 命令

#[tauri::command]
pub fn git_status(app: AppHandle) -> GitStatus {
    let state = app.state::<AppState>();
    let settings = {
        let d = state.data.lock().unwrap();
        d.settings.sync.clone()
    };
    let mut s = state.git.status(&settings);
    let last_sync = {
        let d = state.data.lock().unwrap();
        d.last_sync.clone()
    };
    s.last_sync = last_sync;
    s
}

#[tauri::command]
pub fn git_set_remote(app: AppHandle, sync: SyncSettings) -> Result<GitStatus, String> {
    let state = app.state::<AppState>();
    let data = {
        let mut d = state.data.lock().unwrap();
        d.settings.sync = sync;
        d.clone()
    };
    crate::persist(&app, &data)?;
    Ok(state.git.status(&data.settings.sync))
}

#[tauri::command]
pub fn git_init(app: AppHandle) -> Result<GitStatus, String> {
    let state = app.state::<AppState>();
    let settings = {
        let d = state.data.lock().unwrap();
        d.settings.sync.clone()
    };
    init_repo(&state.dir, &settings)?;
    let message = if settings.commit_message.trim().is_empty() {
        "desk-todo: 初始化数据仓库"
    } else {
        settings.commit_message.trim()
    };
    let _ = state.git.commit_all(&settings, message);
    Ok(state.git.status(&settings))
}

#[tauri::command]
pub fn git_commit(app: AppHandle, message: Option<String>) -> Result<GitResult, String> {
    let state = app.state::<AppState>();
    let settings = {
        let d = state.data.lock().unwrap();
        d.settings.sync.clone()
    };
    let msg = message
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| "desk-todo: 本地提交".to_string());
    let committed = state.git.commit_all(&settings, &msg)?;
    Ok(GitResult {
        ok: true,
        message: if committed {
            "已提交本地改动".into()
        } else {
            "没有需要提交的改动".into()
        },
        committed,
        pushed: false,
        pulled: false,
        status: state.git.status(&settings),
    })
}

#[tauri::command]
pub fn git_pull(app: AppHandle) -> Result<GitResult, String> {
    sync_variant(&app, SyncOp::Pull)
}

#[tauri::command]
pub fn git_push(app: AppHandle) -> Result<GitResult, String> {
    sync_variant(&app, SyncOp::Push)
}

#[tauri::command]
pub fn git_sync_now(app: AppHandle, message: Option<String>) -> Result<GitResult, String> {
    let state = app.state::<AppState>();
    let settings = {
        let d = state.data.lock().unwrap();
        d.settings.sync.clone()
    };
    let msg = message
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| {
            if settings.commit_message.trim().is_empty() {
                "desk-todo: 同步待办数据".to_string()
            } else {
                settings.commit_message.clone()
            }
        });

    let result = state.git.sync_now(&msg, Some(settings), Some(&app))?;
    if result.ok {
        let data = {
            let mut d = state.data.lock().unwrap();
            d.last_sync = Some(crate::model::now_rfc3339());
            d.clone()
        };
        crate::persist(&app, &data)?;
    }
    let _ = app.emit("git://status", result.status.clone());
    Ok(result)
}

#[tauri::command]
pub fn git_test_remote(app: AppHandle, sync: SyncSettings) -> Result<String, String> {
    let state = app.state::<AppState>();
    if git_binary().is_none() {
        return Err("未找到系统 git 命令".into());
    }
    let remote = if sync.remote_name.trim().is_empty() {
        "origin"
    } else {
        sync.remote_name.trim()
    };
    if sync.remote_url.trim().is_empty() {
        return Err("请先填写远程仓库地址".into());
    }
    // 临时把 URL 与当前仓库比较；不修改配置
    let existing = run_git(&state.dir, &sync, &["remote", "get-url", remote]).unwrap_or_default();
    let out = run_git(&state.dir, &sync, &["ls-remote", "--heads", sync.remote_url.trim()])?;
    let _ = &state;
    let count = out.lines().filter(|l| !l.trim().is_empty()).count();
    let same = if existing.trim().is_empty() {
        String::new()
    } else if existing.trim() == sync.remote_url.trim() {
        "（与当前仓库远程地址一致）".to_string()
    } else {
        "（注意：与当前仓库已配置的远程地址不同，同步时会自动更新）".to_string()
    };
    Ok(format!("连接成功，远程有 {count} 个分支{same}"))
}

enum SyncOp {
    Pull,
    Push,
}

fn sync_variant(app: &AppHandle, op: SyncOp) -> Result<GitResult, String> {
    let state = app.state::<AppState>();
    let settings = {
        let d = state.data.lock().unwrap();
        d.settings.sync.clone()
    };
    if git_binary().is_none() {
        return Err("未找到系统 git 命令".into());
    }
    if !state.dir.join(".git").is_dir() {
        init_repo(&state.dir, &settings)?;
    }
    let remote = if settings.remote_name.trim().is_empty() {
        "origin".to_string()
    } else {
        settings.remote_name.trim().to_string()
    };

    let mut result = GitResult {
        ok: false,
        message: String::new(),
        committed: false,
        pushed: false,
        pulled: false,
        status: GitStatus::default(),
    };

    match op {
        SyncOp::Pull => {
            ensure_remote(&state.dir, &settings, &remote)?;
            run_git(&state.dir, &settings, &["fetch", "--prune", &remote])?;
            let upstream = format!("{remote}/{}", settings.branch);
            match run_git(
                &state.dir,
                &settings,
                &["rebase", "--autostash", &upstream],
            ) {
                Ok(_) => {
                    result.pulled = true;
                    result.message = "已拉取远程改动".into();
                }
                Err(e) => {
                    let _ = run_git(&state.dir, &settings, &["rebase", "--abort"]);
                    return Err(format!("拉取失败（可能存在冲突，已中止变基）：{e}"));
                }
            }
        }
        SyncOp::Push => {
            result.committed = state.git.commit_all(&settings, "desk-todo: 提交本地改动")?;
            ensure_remote(&state.dir, &settings, &remote)?;
            let branch = current_branch(&state.dir).unwrap_or_else(|| settings.branch.clone());
            let refspec = format!("HEAD:refs/heads/{branch}");
            run_git(&state.dir, &settings, &["push", "-u", &remote, &refspec])?;
            result.pushed = true;
            result.message = "已推送到远程".into();
        }
    }

    result.ok = true;
    result.status = state.git.status(&settings);
    let _ = app.emit("git://status", result.status.clone());
    Ok(result)
}
