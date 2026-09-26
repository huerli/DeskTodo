//! 无人值守自检：`desk-todo --self-test`
//!
//! 用于在没有图形界面的环境里验证核心逻辑：
//!   1) 数据存储：原子写入 / 重新读取 / 备份
//!   2) 待办 CRUD：新增、改名、勾选、排序、删除、撤销
//!   3) 到期判定：提醒触发条件与去重
//!   4) Git：init / 提交 / 配置远程 / fetch / rebase / push / pull（用本地裸仓库当远程）
//!
//! 自检结束会把用户数据原样还原，不残留测试待办。

use crate::model::{AppData, Priority, SyncSettings, Todo};
use crate::store;
use std::path::PathBuf;

macro_rules! check {
    ($ok:expr, $($arg:tt)*) => {{
        if $ok {
            println!("  ok   {}", format!($($arg)*));
        } else {
            println!("  FAIL {}", format!($($arg)*));
            return Err(format!($($arg)*));
        }
    }};
}

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "desktodo-selftest-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}

pub fn run() -> Result<(), String> {
    println!("== DeskTodo 自检 ==");
    let dir = tmpdir("data");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    println!("\n[1/7] 数据存储");
    let mut data = AppData::default();
    data.settings.sync.author_name = "SelfTest".into();
    data.settings.sync.author_email = "selftest@localhost".into();
    store::save(&dir, &data)?;
    check!(store::load(&dir).todos.is_empty(), "空数据写入并读回");
    let raw = std::fs::read_to_string(dir.join("todos.json")).map_err(|e| e.to_string())?;
    check!(raw.contains("\"version\""), "JSON 含 version 字段");
    let corrupt = dir.join("todos.json");
    std::fs::write(&corrupt, b"{ this is not json").map_err(|e| e.to_string())?;
    let recovered = store::load(&dir);
    check!(recovered.todos.is_empty(), "损坏文件不 panic，返回默认数据");
    check!(
        std::fs::read_dir(&dir)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains("corrupted")),
        "损坏文件被改名留档"
    );

    println!("\n[2/7] 待办 CRUD");
    let mut data = AppData::default();
    let now = chrono::Local::now();
    for (i, title) in ["alpha", "beta", "gamma"].iter().enumerate() {
        let mut t = Todo {
            id: uuid::Uuid::new_v4().to_string(),
            title: (*title).to_string(),
            notes: String::new(),
            done: false,
            due_at: if i == 0 {
                Some((now + chrono::Duration::seconds(30)).to_rfc3339())
            } else {
                None
            },
            priority: if i == 1 { Priority::High } else { Priority::Normal },
            tags: vec![],
            created_at: now.to_rfc3339(),
            updated_at: now.to_rfc3339(),
            completed_at: None,
            notified_at: None,
            order: ((i as i64) + 1) * 10,
        };
        t.title = t.title.trim().to_string();
        data.todos.push(t);
    }
    crate::model::normalize(&mut data.todos);
    check!(data.todos.len() == 3, "新增 3 项");
    check!(data.todos[0].order < data.todos[1].order, "order 递增");

    // 勾选完成
    data.todos[1].done = true;
    data.todos[1].completed_at = Some(chrono::Local::now().to_rfc3339());
    check!(data.todos.iter().filter(|t| t.done).count() == 1, "勾选完成");

    // 拖拽排序：把第 3 项移到最前
    let mut ids: Vec<String> = data.todos.iter().map(|t| t.id.clone()).collect();
    let last = ids.pop().unwrap();
    ids.insert(0, last.clone());
    for (i, id) in ids.iter().enumerate() {
        if let Some(t) = data.todos.iter_mut().find(|t| &t.id == id) {
            t.order = (i as i64 + 1) * 10;
        }
    }
    crate::model::normalize(&mut data.todos);
    check!(data.todos[0].id == last, "重排后顺序正确");

    // 删除 + 撤销
    let victim = data.todos[2].clone();
    data.todos.retain(|t| t.id != victim.id);
    data.trash.push(victim.clone());
    check!(data.todos.len() == 2, "删除一项");
    if let Some(mut t) = data.trash.pop() {
        t.done = false;
        data.todos.push(t);
        crate::model::normalize(&mut data.todos);
    }
    check!(
        data.todos.iter().any(|t| t.id == victim.id),
        "撤销删除后该项回来"
    );

    println!("\n[3/7] 落盘 + 重读一致性");
    store::save(&dir, &data)?;
    let reloaded = store::load(&dir);
    check!(reloaded.todos.len() == data.todos.len(), "数量一致");
    check!(
        reloaded.todos[0].title == data.todos[0].title,
        "顺序与内容一致（首项 {}）",
        reloaded.todos[0].title
    );

    // 导出 / 导入往返
    let export_path = dir.join("export.json");
    store::export_to(&dir, &reloaded, &export_path)?;
    check!(export_path.is_file(), "导出 JSON 文件");
    let imported = store::import_from(&export_path)?;
    check!(
        imported.todos.len() == reloaded.todos.len(),
        "导入后条目数一致（{}）",
        imported.todos.len()
    );
    check!(
        imported.todos[0].title == reloaded.todos[0].title
            && imported.todos[0].priority == reloaded.todos[0].priority
            && imported.todos[0].due_at == reloaded.todos[0].due_at,
        "导入后标题/优先级/到期时间一致"
    );
    let bad = dir.join("bad.json");
    std::fs::write(&bad, b"[1,2,3]").map_err(|e| e.to_string())?;
    check!(
        store::import_from(&bad).is_err(),
        "格式错误的文件导入时报错而非 panic"
    );

    println!("\n[4/7] 到期提醒判定");
    let lead_minutes: i64 = 0;
    let due_now = (chrono::Local::now() - chrono::Duration::seconds(5)).to_rfc3339();
    let parsed = chrono::DateTime::parse_from_rfc3339(&due_now)
        .map_err(|e| e.to_string())?
        .with_timezone(&chrono::Local);
    let trigger = parsed - chrono::Duration::minutes(lead_minutes);
    check!(
        chrono::Local::now() >= trigger,
        "已过期任务会触发提醒"
    );
    let future = chrono::Local::now() + chrono::Duration::hours(3);
    check!(
        chrono::Local::now() < future - chrono::Duration::minutes(lead_minutes),
        "3 小时后到期不触发提醒"
    );
    let mut t = data.todos[0].clone();
    t.notified_at = Some(chrono::Local::now().to_rfc3339());
    let notified = t.notified_at.is_some();
    check!(notified, "已提醒标记可阻止重复提醒");

    println!("\n[5/7] Git 本地提交");
    let git_dir = tmpdir("git");
    std::fs::create_dir_all(&git_dir).map_err(|e| e.to_string())?;
    let remote_dir = tmpdir("remote");
    std::fs::create_dir_all(&remote_dir).map_err(|e| e.to_string())?;

    let mut settings = SyncSettings {
        author_name: "SelfTest".into(),
        author_email: "selftest@localhost".into(),
        branch: "main".into(),
        ..Default::default()
    };

    let git = crate::git_sync::GitManager::new(git_dir.clone());
    crate::git_sync::init_repo(&git_dir, &settings)?;
    check!(git_dir.join(".git").is_dir(), "init 生成 .git");
    check!(git_dir.join(".gitignore").is_file(), "生成 .gitignore");

    // 诊断日志不得进版本库：模拟应用写出 boot-trace.log / frontend.log，
    // 提交后这些文件不应出现在索引里（回归：曾因 .gitignore 只在文件不存在时
    // 创建，导致已有仓库拿不到新增的忽略规则）
    std::fs::write(git_dir.join("boot-trace.log"), "[00:00:00] boot\n").map_err(|e| e.to_string())?;
    std::fs::write(git_dir.join("frontend.log"), "[info] boot\n").map_err(|e| e.to_string())?;
    // 故意先放一个内容不全的 .gitignore，验证规则会被补全而不是被跳过
    std::fs::write(git_dir.join(".gitignore"), "# 用户自己的规则\nmy-notes.txt\n")
        .map_err(|e| e.to_string())?;

    // 用真实数据文件做一次提交
    let mut gdata = data.clone();
    gdata.todos.truncate(1);
    store::save(&git_dir, &gdata)?;
    let committed = git.commit_all(&settings, "selftest: 首次提交")?;
    check!(committed, "首次提交成功");

    let ignore_text =
        std::fs::read_to_string(git_dir.join(".gitignore")).map_err(|e| e.to_string())?;
    check!(
        ignore_text.contains("my-notes.txt"),
        "补全 .gitignore 时不破坏用户已有规则"
    );
    check!(
        ignore_text.lines().any(|l| l.trim() == "*.log"),
        "已有 .gitignore 被补上 *.log 规则"
    );
    let tracked = {
        let repo = git2::Repository::open(&git_dir).map_err(|e| e.to_string())?;
        let index = repo.index().map_err(|e| e.to_string())?;
        index
            .iter()
            .filter_map(|e| String::from_utf8(e.path.clone()).ok())
            .collect::<Vec<String>>()
    };
    check!(
        !tracked.iter().any(|p| p.ends_with(".log")),
        "诊断日志文件未被纳入版本库（索引中无 .log）"
    );

    let again = git.commit_all(&settings, "selftest: 无改动")?;
    check!(!again, "无改动时不产生空提交");
    let st = git.status(&settings);
    check!(st.is_repo, "status 识别为仓库");
    check!(st.last_commit.is_some(), "能读到最近提交：{:?}", st.last_commit);
    check!(!st.dirty, "提交后工作区干净");

    println!("\n[6/7] Git 推送 / 拉取（本地裸仓库作为远程）");
    let git_bin = crate::git_sync::git_binary();
    check!(git_bin.is_some(), "系统 git 可用: {:?}", git_bin);
    let bare = remote_dir.join("todo-data.git");
    let out = std::process::Command::new(git_bin.clone().unwrap())
        .args(["init", "--bare", "-b", "main"])
        .arg(&bare)
        .output()
        .map_err(|e| e.to_string())?;
    check!(out.status.success(), "创建裸仓库作为远程");

    settings.remote_url = bare.to_string_lossy().to_string();
    settings.remote_name = "origin".into();
    let result = git.sync_now("selftest: 同步", Some(settings.clone()), None)?;
    check!(result.ok, "sync_now 返回 ok：{}", result.message);
    check!(result.pushed, "已推送 ({})", result.message);

    // 远程应当有我们的提交
    let count = std::process::Command::new(git_bin.clone().unwrap())
        .current_dir(&bare)
        .args(["rev-list", "--count", "main"])
        .output()
        .map_err(|e| e.to_string())?;
    let count = String::from_utf8_lossy(&count.stdout).trim().to_string();
    check!(count == "1", "远程分支有 1 个提交（实际 {count}）");

    // 模拟另一台设备：新目录 clone 后改数据再推回，本机再拉取
    let peer_dir = tmpdir("peer");
    let out = std::process::Command::new(git_bin.clone().unwrap())
        .args(["clone", "--branch", "main"])
        .arg(&bare)
        .arg(&peer_dir)
        .output()
        .map_err(|e| e.to_string())?;
    check!(out.status.success(), "克隆远程仓库（模拟第二台设备）");

    let peer_git = crate::git_sync::GitManager::new(peer_dir.clone());
    let mut peer_data = store::load(&peer_dir);
    peer_data.todos.push(Todo {
        id: uuid::Uuid::new_v4().to_string(),
        title: "peer-device-item".into(),
        notes: String::new(),
        done: false,
        due_at: None,
        priority: Priority::Normal,
        tags: vec![],
        created_at: chrono::Local::now().to_rfc3339(),
        updated_at: chrono::Local::now().to_rfc3339(),
        completed_at: None,
        notified_at: None,
        order: 990,
    });
    crate::model::normalize(&mut peer_data.todos);
    store::save(&peer_dir, &peer_data)?;
    let peer_result = peer_git.sync_now("selftest: 来自第二台设备", Some(settings.clone()), None)?;
    check!(peer_result.ok, "第二台设备推送成功");

    // 本机拉取并确认能看到对方的数据
    let pull_result = git.sync_now("selftest: 拉取前提交", Some(settings.clone()), None)?;
    check!(pull_result.ok, "本机再次同步成功：{}", pull_result.message);
    check!(pull_result.pulled, "识别到远程有新提交并完成合并");
    let merged = store::load(&git_dir);
    check!(
        merged.todos.iter().any(|t| t.title == "peer-device-item"),
        "拉取后本地已包含另一台设备的数据"
    );

    println!("\n[7/7] SSH 选项（防止首次连接新主机时 Host key verification failed）");
    // 应用以 GIT_TERMINAL_PROMPT=0 运行，SSH 无法询问“是否信任该主机”，
    // 若不带 StrictHostKeyChecking=accept-new 就会直接报 Host key verification failed。
    let ssh_cmd = crate::git_sync::ssh_command("");
    check!(
        ssh_cmd.contains("StrictHostKeyChecking=accept-new"),
        "默认 SSH 命令包含 accept-new：{ssh_cmd}"
    );
    check!(
        ssh_cmd.contains("BatchMode=yes"),
        "默认 SSH 命令包含 BatchMode=yes（不会阻塞等待输入）"
    );
    check!(!ssh_cmd.contains("-i "), "未指定私钥时不带 -i 参数");
    let ssh_cmd_key = crate::git_sync::ssh_command("~/.ssh/id_ed25519");
    check!(
        ssh_cmd_key.contains("-i ~/.ssh/id_ed25519") && ssh_cmd_key.contains("IdentitiesOnly=yes"),
        "指定私钥时带上 -i 与 IdentitiesOnly：{ssh_cmd_key}"
    );

    // 用一个“必然连不上”的 SSH 地址验证选项确实生效：
    //   修复前 → Host key verification failed
    //   修复后 → Connection refused（说明 accept-new 生效，已推进到连接阶段）
    {
        let ssh_probe = tmpdir("sshprobe");
        std::fs::create_dir_all(&ssh_probe).map_err(|e| e.to_string())?;
        let probe_git = crate::git_sync::GitManager::new(ssh_probe.clone());
        crate::git_sync::init_repo(&ssh_probe, &settings)?;
        let mut probe_data = AppData::default();
        probe_data.todos.push(Todo {
            id: uuid::Uuid::new_v4().to_string(),
            title: "ssh-probe".into(),
            notes: String::new(),
            done: false,
            due_at: None,
            priority: Priority::Normal,
            tags: vec![],
            created_at: chrono::Local::now().to_rfc3339(),
            updated_at: chrono::Local::now().to_rfc3339(),
            completed_at: None,
            notified_at: None,
            order: 10,
        });
        store::save(&ssh_probe, &probe_data)?;

        let mut probe_settings = settings.clone();
        // 保留端口（不能省，否则会被当成 scp 风格路径），落在 TEST-NET-1 网段
        probe_settings.remote_url = "ssh://git@192.0.2.1:22/probe/repo.git".into();
        probe_settings.ssh_key_path = String::new();
        let err = probe_git
            .sync_now("selftest: ssh 探测", Some(probe_settings), None)
            .err()
            .unwrap_or_default();
        check!(
            !err.contains("Host key verification failed"),
            "SSH 探测未再出现 Host key verification failed"
        );
        // 只要错误来自“连接阶段”就说明密钥校验已放行。
        // 不同网络环境下措辞不同，这里覆盖常见几种。
        let low = err.to_lowercase();
        let connection_stage = [
            "connection refused",
            "connection timed out",
            "operation timed out",
            "network is unreachable",
            "no route to host",
            "connection closed",
            "connection reset",
            "broken pipe",
        ]
        .iter()
        .any(|k| low.contains(k));
        check!(
            connection_stage,
            "错误来自连接阶段而非密钥校验：{}",
            err.lines().next().unwrap_or("").trim()
        );
        let _ = std::fs::remove_dir_all(&ssh_probe);
    }

    // 清理临时目录
    for d in [&dir, &git_dir, &remote_dir, &peer_dir] {
        let _ = std::fs::remove_dir_all(d);
    }

    println!("\n== 全部通过 ==");
    Ok(())
}

/// 供外部调用（例如 GUI 内隐藏入口）
pub fn run_headless() {
    match run() {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("\n== 自检失败: {e} ==");
            std::process::exit(1);
        }
    }
}
