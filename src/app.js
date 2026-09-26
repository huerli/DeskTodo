/* ============================================================
   DeskTodo 前端逻辑
   - 通过 window.__TAURI__（withGlobalTauri）调用 Rust 命令
   - 自研 pointer 拖拽：标题栏拖动窗口，列表项拖动排序
   ============================================================ */

const T = window.__TAURI__ ?? null;
const rawInvoke = T?.core?.invoke ?? (async () => {
  throw new Error("Tauri API 不可用（请通过 DeskTodo 应用打开）");
});

/**
 * 带重试的命令调用。
 * WebView 刚加载时 IPC 通道可能尚未就绪，此时发出的调用会被静默丢弃
 * （既不 resolve 也不 reject），因此这里对超时做重试。
 */
async function invoke(cmd, args = {}, { retries = 4, timeoutMs = 2500 } = {}) {
  let lastErr = null;
  for (let attempt = 0; attempt <= retries; attempt++) {
    try {
      return await Promise.race([
        rawInvoke(cmd, args),
        new Promise((_, rej) =>
          setTimeout(() => rej(new Error(`IPC_TIMEOUT:${cmd}`)), timeoutMs)
        ),
      ]);
    } catch (e) {
      lastErr = e;
      const msg = String(e?.message ?? e);
      // 真正的业务错误直接抛出；只有 IPC 未就绪/超时才重试
      if (!msg.startsWith("IPC_TIMEOUT")) throw e;
      await new Promise((r) => setTimeout(r, 150 * (attempt + 1)));
    }
  }
  throw lastErr ?? new Error(`调用 ${cmd} 失败`);
}

const listen = T?.event?.listen ?? (async () => () => {});
const openPath = T?.opener?.openPath ?? null;

// 启动轨迹（写入 boot-trace.log），排查"界面空白"时非常有用
const trace = (step) => {
  try {
    rawInvoke("trace", { step: String(step) }).catch(() => {});
  } catch {}
};

/* ---------------------------------------------------------- 状态 */

let data = null;                 // AppData 快照
let filter = "all";              // all | active | done
let newPriority = "normal";      // 新增时的优先级
let toastTimer = null;
let confirmResolve = null;

/* ---------------------------------------------------------- 日志与错误上报 */

// 前端异常写回应用数据目录的 frontend.log，方便排查"界面空白"类问题
function logOnly(level, message) {
  const text = String(message).slice(0, 2000);
  invoke("log_frontend", { level, message: text }).catch(() => {});
}

function report(level, message) {
  const text = String(message).slice(0, 2000);
  if (level === "error") console.error(text);
  logOnly(level, text);
}

const logInfo = (m) => report("info", m);
const logError = (m) => report("error", m);

function installLogging() {
  window.addEventListener("error", (e) => {
    const where = e.filename ? ` @${e.filename}:${e.lineno}:${e.colno}` : "";
    logOnly("error", `uncaught: ${e.message}${where}`);
  });
  window.addEventListener("unhandledrejection", (e) => {
    const r = e.reason;
    logOnly("error", `unhandled rejection: ${r?.message ?? r}`);
  });
  const origError = console.error.bind(console);
  console.error = (...args) => {
    origError(...args);
    try {
      logOnly("warn", args.map(String).join(" "));
    } catch {}
  };
}

const el = (id) => document.getElementById(id);
const $ = {
  widget: el("widget"),
  titlebar: el("titlebar"),
  titleText: el("title-text"),
  countPill: el("count-pill"),
  collapsedBar: el("collapsed-bar"),
  collapseDot: el("collapse-dot"),
  collapseText: el("collapse-text"),
  body: el("body"),
  summaryText: el("summary-text"),
  list: el("list"),
  empty: el("empty"),
  emptyTitle: el("empty-title"),
  emptySub: el("empty-sub"),
  inputTitle: el("input-title"),
  inputDue: el("input-due"),
  prioGroup: el("prio-group"),
  settings: el("settings"),
  toast: el("toast"),
  confirm: el("confirm"),
  confirmText: el("confirm-text"),
  filterSeg: el("filter-seg"),
};

const PRIORITY_LABEL = { high: "高", normal: "", low: "低" };

/* ---------------------------------------------------------- 工具函数 */

function toast(msg, isErr = false) {
  $.toast.textContent = msg;
  $.toast.classList.toggle("err", !!isErr);
  $.toast.classList.remove("hidden");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => $.toast.classList.add("hidden"), isErr ? 4200 : 2000);
}

function confirmBox(text) {
  $.confirmText.textContent = text;
  $.confirm.classList.remove("hidden");
  return new Promise((resolve) => (confirmResolve = resolve));
}

function closeConfirm(v) {
  $.confirm.classList.add("hidden");
  const r = confirmResolve;
  confirmResolve = null;
  if (r) r(v);
}

// WKWebView 不实现 window.prompt，这里用自绘弹窗
function askBox(text, initial = "", placeholder = "") {
  el("prompt-text").textContent = text;
  const input = el("prompt-input");
  input.value = initial;
  input.placeholder = placeholder;
  el("prompt").classList.remove("hidden");
  setTimeout(() => {
    input.focus();
    input.select();
  }, 30);
  return new Promise((resolve) => {
    const done = (v) => {
      el("prompt").classList.add("hidden");
      el("prompt-yes").removeEventListener("click", onYes);
      input.removeEventListener("keydown", onKey);
      resolve(v);
    };
    const onYes = () => done(input.value);
    const onKey = (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        done(input.value);
      } else if (e.key === "Escape") {
        done(null);
      }
    };
    el("prompt-yes").addEventListener("click", onYes);
    el("prompt-no").addEventListener("click", () => done(null), { once: true });
    input.addEventListener("keydown", onKey);
  });
}

// 与 Rust 端 chrono::Local::now().to_rfc3339() 对齐：带本地偏移的 RFC3339
function toRfc3339(d) {
  const pad = (n, w = 2) => String(Math.abs(n)).padStart(w, "0");
  const off = -d.getTimezoneOffset();
  const sign = off >= 0 ? "+" : "-";
  const oh = pad(Math.floor(Math.abs(off) / 60));
  const om = pad(Math.abs(off) % 60);
  return (
    `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}` +
    `T${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}` +
    `.${pad(d.getMilliseconds(), 3)}${sign}${oh}:${om}`
  );
}

function fromDatetimeLocal(v) {
  if (!v) return null;
  const d = new Date(v);
  if (Number.isNaN(d.getTime())) return null;
  return toRfc3339(d);
}

/**
 * 宽松解析用户手写的时间：
 *   2026-03-05 14:30 / 2026/3/5 14:30 / 2026-03-05 / 14:30（今天）
 */
function parseLooseDateTime(text) {
  const s = String(text).trim().replace(/\//g, "-").replace(/\s+/g, " ");
  let m = s.match(/^(\d{4})-(\d{1,2})-(\d{1,2})(?:[ T](\d{1,2}):(\d{2}))?$/);
  if (m) {
    const d = new Date(
      Number(m[1]), Number(m[2]) - 1, Number(m[3]),
      m[4] ? Number(m[4]) : 9, m[5] ? Number(m[5]) : 0, 0, 0
    );
    return Number.isNaN(d.getTime()) ? null : toRfc3339(d);
  }
  m = s.match(/^(\d{1,2}):(\d{2})$/);
  if (m) {
    const d = new Date();
    d.setHours(Number(m[1]), Number(m[2]), 0, 0);
    return toRfc3339(d);
  }
  m = s.match(/^(\d{1,2})-(\d{1,2})(?:[ T](\d{1,2}):(\d{2}))?$/);
  if (m) {
    const now = new Date();
    const d = new Date(
      now.getFullYear(), Number(m[1]) - 1, Number(m[2]),
      m[3] ? Number(m[3]) : 9, m[4] ? Number(m[4]) : 0, 0, 0
    );
    return Number.isNaN(d.getTime()) ? null : toRfc3339(d);
  }
  return null;
}

function toDatetimeLocalValue(iso) {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const pad = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function startOfToday() {
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  return d;
}

function relTime(iso) {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const now = new Date();
  const diffMs = d - now;
  const abs = Math.abs(diffMs);
  const mins = Math.round(abs / 60000);
  const label = (v) => (diffMs >= 0 ? `${v}后` : `${v}前`);

  if (abs < 60000) return diffMs >= 0 ? "即将到期" : "刚刚到期";
  if (mins < 60) return label(`${mins} 分钟`);
  const hours = Math.round(mins / 60);
  if (hours < 24 && sameDay(d, now)) return label(`${hours} 小时`);
  const days = Math.round(hours / 24);
  if (days === 1) return diffMs >= 0 ? "明天" : "昨天";
  if (days < 7) return label(`${days} 天`);
  return `${d.getMonth() + 1}月${d.getDate()}日`;
}

function sameDay(a, b) {
  return (
    a.getFullYear() === b.getFullYear() &&
    a.getMonth() === b.getMonth() &&
    a.getDate() === b.getDate()
  );
}

function dueClass(t) {
  if (!t.due_at || t.done) return "";
  const d = new Date(t.due_at);
  if (Number.isNaN(d.getTime())) return "";
  const now = Date.now();
  if (d.getTime() < now) return "overdue";
  if (d.getTime() - now < 3600_000) return "soon";
  return "";
}

function applyTheme() {
  const t = data?.settings?.window?.theme ?? "dark";
  const resolved =
    t === "auto"
      ? window.matchMedia?.("(prefers-color-scheme: light)").matches
        ? "light"
        : "dark"
      : t;
  document.documentElement.dataset.theme = resolved;
  const op = data?.settings?.window?.opacity ?? 0.96;
  document.documentElement.style.setProperty("--alpha", String(op));
}

/* ---------------------------------------------------------- 排序 / 过滤 */

function visibleTodos() {
  const list = [...(data?.todos ?? [])];
  const mode = data?.settings?.sort ?? "manual";
  if (mode === "due") {
    list.sort((a, b) => {
      if (!a.due_at && !b.due_at) return a.order - b.order;
      if (!a.due_at) return 1;
      if (!b.due_at) return -1;
      return new Date(a.due_at) - new Date(b.due_at);
    });
  } else if (mode === "priority") {
    const w = { high: 0, normal: 1, low: 2 };
    list.sort((a, b) => (w[a.priority] ?? 1) - (w[b.priority] ?? 1) || a.order - b.order);
  } else if (mode === "created") {
    list.sort((a, b) => new Date(b.created_at) - new Date(a.created_at));
  } else {
    list.sort((a, b) => a.order - b.order);
  }
  // 已完成的沉底
  list.sort((a, b) => Number(a.done) - Number(b.done));
  if (filter === "active") return list.filter((t) => !t.done);
  if (filter === "done") return list.filter((t) => t.done);
  return list;
}

/* ---------------------------------------------------------- 渲染 */

function render() {
  if (!data) return;
  const todos = data.todos ?? [];
  const pending = todos.filter((t) => !t.done);
  const done = todos.length - pending.length;
  const overdue = pending.filter((t) => dueClass(t) === "overdue").length;
  const today = pending.filter((t) => t.due_at && sameDay(new Date(t.due_at), new Date())).length;

  // 标题栏计数
  $.countPill.textContent = String(pending.length);
  $.countPill.classList.toggle("overdue", overdue > 0);

  // 摘要
  const parts = [];
  if (pending.length === 0 && done === 0) parts.push("还没有待办");
  else {
    if (pending.length) parts.push(`${pending.length} 项待办`);
    if (today) parts.push(`今天 ${today} 项`);
    if (overdue) parts.push(`${overdue} 项逾期`);
    if (done) parts.push(`已完成 ${done} 项`);
  }
  $.summaryText.textContent = parts.join(" · ");

  // 收起态摘要
  if (todos.length === 0) {
    $.collapseText.textContent = "暂无待办";
    $.collapseDot.className = "dot";
  } else {
    $.collapseText.textContent = overdue
      ? `${pending.length} 项待办 · ${overdue} 项逾期`
      : `${pending.length} 项待办 · 已完成 ${done}`;
    $.collapseDot.className = "dot " + (overdue ? "danger" : pending.length ? "warn" : "");
  }

  // 列表
  const list = visibleTodos();
  $.list.innerHTML = "";
  const frag = document.createDocumentFragment();
  for (const t of list) frag.appendChild(renderItem(t));
  $.list.appendChild(frag);

  const showEmpty = list.length === 0;
  $.empty.classList.toggle("hidden", !showEmpty);
  $.list.classList.toggle("hidden", showEmpty);
  if (showEmpty) {
    if (filter === "done") {
      $.emptyTitle.textContent = "还没有已完成的事项";
      $.emptySub.textContent = "勾选左侧圆圈即可完成";
    } else if (todos.length > 0) {
      $.emptyTitle.textContent = "当前筛选下没有内容";
      $.emptySub.textContent = "切回「全部」看看";
    } else {
      $.emptyTitle.textContent = "保持桌面清爽";
      $.emptySub.textContent = "在下面输入内容，回车即可添加";
    }
  }

  applyTheme();
  syncSettingsUI();
}

function renderItem(t) {
  const li = document.createElement("li");
  li.className = "item" + (t.done ? " done" : "");
  const dc = dueClass(t);
  if (dc) li.classList.add(dc);
  li.dataset.id = t.id;

  // 拖拽手柄
  const grip = document.createElement("div");
  grip.className = "grip";
  grip.title = "拖动排序";
  grip.innerHTML =
    '<svg viewBox="0 0 24 24" class="ico" style="width:11px;height:11px"><circle cx="9" cy="6" r="1.4"/><circle cx="15" cy="6" r="1.4"/><circle cx="9" cy="12" r="1.4"/><circle cx="15" cy="12" r="1.4"/><circle cx="9" cy="18" r="1.4"/><circle cx="15" cy="18" r="1.4"/></svg>';
  li.appendChild(grip);

  // 勾选
  const check = document.createElement("button");
  check.className = "check";
  check.title = t.done ? "标记为未完成" : "标记为完成";
  check.innerHTML = '<svg viewBox="0 0 24 24" class="ico"><path d="M5 13l4 4L19 7"/></svg>';
  check.addEventListener("click", async (e) => {
    e.stopPropagation();
    await run(() => invoke("toggle_todo", { id: t.id, done: !t.done }));
  });
  li.appendChild(check);

  // 内容
  const main = document.createElement("div");
  main.className = "item-main";

  const title = document.createElement("input");
  title.className = "item-title";
  title.value = t.title;
  title.spellcheck = false;
  title.addEventListener("keydown", async (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      title.blur();
    } else if (e.key === "Escape") {
      title.value = t.title;
      title.blur();
    }
  });
  title.addEventListener("blur", async () => {
    const v = title.value.trim();
    if (!v) {
      title.value = t.title;
      return;
    }
    if (v !== t.title) await run(() => invoke("update_todo", { payload: { id: t.id, title: v } }));
  });
  main.appendChild(title);

  // 元信息行
  const metaBits = [];
  if (PRIORITY_LABEL[t.priority]) {
    metaBits.push(`<span class="badge ${t.priority}">${PRIORITY_LABEL[t.priority]}优先</span>`);
  }
  if (t.due_at) {
    const cls = t.priority === "high" ? "badge" : "badge";
    metaBits.push(`<span class="due ${cls}">⏰ ${relTime(t.due_at)}</span>`);
  }
  for (const tag of t.tags ?? []) metaBits.push(`<span class="badge tag">#${escapeHtml(tag)}</span>`);
  if (metaBits.length) {
    const meta = document.createElement("div");
    meta.className = "item-meta";
    meta.innerHTML = metaBits.join("");
    main.appendChild(meta);
  }
  li.appendChild(main);

  // 操作
  const actions = document.createElement("div");
  actions.className = "item-actions";

  const btnEdit = document.createElement("button");
  btnEdit.className = "icon-btn";
  btnEdit.title = "设置到期时间";
  btnEdit.innerHTML =
    '<svg viewBox="0 0 24 24" class="ico"><circle cx="12" cy="12" r="8.5"/><path d="M12 7.5V12l3 2"/></svg>';
  btnEdit.addEventListener("click", async (e) => {
    e.stopPropagation();
    const cur = toDatetimeLocalValue(t.due_at).replace("T", " ");
    const input = await askBox("到期时间（留空表示清除）", cur, "2026-03-05 14:30");
    if (input === null) return;
    const trimmed = String(input).trim();
    if (!trimmed) {
      await run(() => invoke("update_todo", { payload: { id: t.id, due_at: null } }));
      return;
    }
    const iso = parseLooseDateTime(trimmed);
    if (!iso) return toast("时间格式不正确，示例：2026-03-05 14:30", true);
    await run(() => invoke("update_todo", { payload: { id: t.id, due_at: iso } }));
  });
  actions.appendChild(btnEdit);

  const btnDel = document.createElement("button");
  btnDel.className = "icon-btn";
  btnDel.title = "删除（可用 ⌘/Ctrl+Z 撤销）";
  btnDel.innerHTML =
    '<svg viewBox="0 0 24 24" class="ico"><path d="M4 7h16M9 7V5h6v2M6 7l1 13h10l1-13"/></svg>';
  btnDel.addEventListener("click", async (e) => {
    e.stopPropagation();
    await run(() => invoke("delete_todo", { id: t.id }));
    toast("已删除，按 ⌘/Ctrl+Z 可撤销");
  });
  actions.appendChild(btnDel);

  li.appendChild(actions);
  return li;
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

/* ---------------------------------------------------------- 设置面板同步 */

function syncSettingsUI() {
  const s = data.settings;
  const set = (id, v, prop = "checked") => {
    const node = el(id);
    if (node) node[prop] = v;
  };
  set("s-reminder-enabled", s.reminder.enabled);
  el("s-lead").value = String(s.reminder.lead_minutes);
  set("s-sound", s.reminder.sound);
  set("s-voice", s.reminder.voice);
  set("s-on-top", s.window.always_on_top);
  el("s-theme").value = s.window.theme;
  set("s-start-hidden", s.start_hidden);
  el("s-sort").value = s.sort;

  el("s-git-url").value = s.sync.remote_url ?? "";
  el("s-git-remote").value = s.sync.remote_name ?? "origin";
  el("s-git-branch").value = s.sync.branch ?? "main";
  el("s-git-name").value = s.sync.author_name ?? "";
  el("s-git-email").value = s.sync.author_email ?? "";
  el("s-git-user").value = s.sync.username ?? "";
  el("s-git-token").value = s.sync.token ?? "";
  // 未填私钥时，用文案说明默认走 ~/.ssh/config 的别名与默认密钥
  el("s-git-ssh").value =
    s.sync.ssh_key_path && s.sync.ssh_key_path.trim()
      ? s.sync.ssh_key_path
      : "ssh 默认配置（~/.ssh/config + 约定密钥）";
  set("s-git-auto", s.sync.auto_commit);
  el("s-git-msg").value = s.sync.commit_message ?? "";
}

function collectSettings() {
  const s = JSON.parse(JSON.stringify(data.settings));
  s.reminder.enabled = el("s-reminder-enabled").checked;
  s.reminder.lead_minutes = Number(el("s-lead").value);
  s.reminder.sound = el("s-sound").checked;
  s.reminder.voice = el("s-voice").checked;
  s.window.always_on_top = el("s-on-top").checked;
  s.window.theme = el("s-theme").value;
  s.start_hidden = el("s-start-hidden").checked;
  s.sort = el("s-sort").value;

  s.sync.remote_url = el("s-git-url").value.trim();
  s.sync.remote_name = el("s-git-remote").value.trim() || "origin";
  s.sync.branch = el("s-git-branch").value.trim() || "main";
  s.sync.author_name = el("s-git-name").value.trim();
  s.sync.author_email = el("s-git-email").value.trim();
  s.sync.username = el("s-git-user").value.trim();
  s.sync.token = el("s-git-token").value.trim();
  // 提示文案不算私钥路径，避免被误存成 -i 参数
  s.sync.ssh_key_path = el("s-git-ssh").value.trim().startsWith("ssh ") ? "" : el("s-git-ssh").value.trim();
  s.sync.auto_commit = el("s-git-auto").checked;
  s.sync.commit_message = el("s-git-msg").value.trim() || "desk-todo: 同步待办数据";
  return s;
}

async function saveSettings(showToast = false) {
  const payload = collectSettings();
  await run(async () => {
    const d = await invoke("save_settings", { settings: payload });
    return d;
  });
  if (showToast) toast("设置已保存");
}

function debounce(fn, ms) {
  let h = null;
  return (...args) => {
    clearTimeout(h);
    h = setTimeout(() => fn(...args), ms);
  };
}
const saveSettingsSoon = debounce(() => saveSettings(false).catch(() => {}), 400);

/** 时间胶囊的选中态与清除按钮显隐 */
function syncTimeField() {
  const has = !!$.inputDue.value;
  el("time-field").classList.toggle("has-value", has);
  el("btn-clear-time").classList.toggle("hidden", !has);
}

/* ---------------------------------------------------------- 命令包装 */

async function run(fn) {
  try {
    const next = await fn();
    if (next && next.todos) {
      data = next;
      render();
    }
    return next;
  } catch (e) {
    toast(typeof e === "string" ? e : e?.message ?? String(e), true);
    return null;
  }
}

async function reload() {
  data = await invoke("get_state");
  render();
}

/* ---------------------------------------------------------- 窗口拖动 / 排序 */

function installPointerDrag() {
  // ---- 标题栏拖动窗口 ----
  let winDrag = null;
  const titlebar = $.titlebar;

  titlebar.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    if (e.target.closest("button")) return; // 点按钮不拖动
    winDrag = { lastX: e.clientX, lastY: e.clientY, dx: 0, dy: 0, raf: 0 };
    titlebar.setPointerCapture(e.pointerId);
    document.body.style.cursor = "grabbing";
  });

  titlebar.addEventListener("pointermove", (e) => {
    if (!winDrag) return;
    winDrag.dx += e.clientX - winDrag.lastX;
    winDrag.dy += e.clientY - winDrag.lastY;
    winDrag.lastX = e.clientX;
    winDrag.lastY = e.clientY;
    scheduleWindowMove();
  });

  const endWinDrag = async (e) => {
    if (!winDrag) return;
    const moved = Math.abs(winDrag.dx) + Math.abs(winDrag.dy) > 1;
    cancelAnimationFrame(winDrag.raf);
    winDrag = null;
    document.body.style.cursor = "";
    try {
      titlebar.releasePointerCapture(e.pointerId);
    } catch {}
    if (moved) await persistWindowState();
  };
  titlebar.addEventListener("pointerup", endWinDrag);
  titlebar.addEventListener("pointercancel", endWinDrag);

  async function scheduleWindowMove() {
    if (!winDrag || winDrag.raf) return;
    winDrag.raf = requestAnimationFrame(async () => {
      if (!winDrag) return;
      const { dx, dy } = winDrag;
      winDrag.dx = 0;
      winDrag.dy = 0;
      winDrag.raf = 0;
      try {
        await invoke("move_window", { dx, dy });
      } catch {}
    });
  }

  // ---- 列表项拖动排序 ----
  $.list.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    const grip = e.target.closest(".grip");
    if (!grip) return;
    const li = grip.closest(".item");
    if (!li) return;

    if ((data?.settings?.sort ?? "manual") !== "manual") {
      toast("当前是自动排序，切到「手动」后才能拖拽");
      return;
    }

    const startY = e.clientY;
    let dragging = false;
    let placeholder = null;
    let targetIndex = -1;
    grip.setPointerCapture(e.pointerId);

    const onMove = (ev) => {
      const dy = ev.clientY - startY;
      if (!dragging && Math.abs(dy) < 4) return;
      if (!dragging) {
        dragging = true;
        li.classList.add("dragging");
        placeholder = li.cloneNode(true);
        placeholder.style.opacity = "0.25";
        placeholder.classList.remove("dragging");
      }

      const items = [...$.list.querySelectorAll(".item")].filter((n) => n !== li);
      let idx = items.length;
      for (let i = 0; i < items.length; i++) {
        const r = items[i].getBoundingClientRect();
        if (ev.clientY < r.top + r.height / 2) {
          idx = i;
          break;
        }
      }
      targetIndex = idx;

      items.forEach((n) => n.classList.remove("drop-before", "drop-after"));
      if (items[idx]) items[idx].classList.add("drop-before");
      else if (items[items.length - 1]) items[items.length - 1].classList.add("drop-after");
    };

    const onUp = async (ev) => {
      grip.removeEventListener("pointermove", onMove);
      grip.removeEventListener("pointerup", onUp);
      grip.removeEventListener("pointercancel", onUp);
      try {
        grip.releasePointerCapture(ev.pointerId);
      } catch {}
      if (!dragging) return;
      li.classList.remove("dragging");
      $.list.querySelectorAll(".item").forEach((n) => n.classList.remove("drop-before", "drop-after"));

      const ids = [...$.list.querySelectorAll(".item")].map((n) => n.dataset.id);
      const from = ids.indexOf(li.dataset.id);
      if (from < 0) return;
      ids.splice(from, 1);
      const insertAt = Math.max(0, Math.min(targetIndex, ids.length));
      ids.splice(insertAt, 0, li.dataset.id);
      // 已完成项在渲染时被沉底，手动模式下按拖拽结果重排 order 即可
      await run(() => invoke("reorder_todos", { ids }));
    };

    grip.addEventListener("pointermove", onMove);
    grip.addEventListener("pointerup", onUp);
    grip.addEventListener("pointercancel", onUp);
  });

  // ---- 窗口尺寸变化后记忆 ----
  let resizeTimer = null;
  window.addEventListener("resize", () => {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(persistWindowState, 500);
  });
}

async function persistWindowState() {
  // 收起状态下不写尺寸，避免把收起高度当成展开高度记住
  const collapsed = data?.settings?.window?.collapsed ?? false;
  try {
    await invoke("save_window_state", {
      x: null,
      y: null,
      width: collapsed ? null : window.innerWidth,
      height: collapsed ? null : window.innerHeight,
    });
  } catch {}
}

/* ---------------------------------------------------------- 提醒 / 语音 */

function speak(text) {
  if (!data?.settings?.reminder?.voice) return;
  if (!("speechSynthesis" in window)) return;
  try {
    const u = new SpeechSynthesisUtterance(text);
    u.lang = navigator.language || "zh-CN";
    u.rate = 1.02;
    window.speechSynthesis.speak(u);
  } catch {}
}

function beep() {
  if (!data?.settings?.reminder?.sound) return;
  try {
    const Ctx = window.AudioContext || window.webkitAudioContext;
    if (!Ctx) return;
    const ctx = new Ctx();
    const osc = ctx.createOscillator();
    const gain = ctx.createGain();
    osc.type = "sine";
    osc.frequency.value = 880;
    gain.gain.value = 0.08;
    osc.connect(gain).connect(ctx.destination);
    osc.start();
    osc.frequency.setValueAtTime(1174, ctx.currentTime + 0.12);
    osc.stop(ctx.currentTime + 0.28);
    setTimeout(() => ctx.close(), 600);
  } catch {}
}

/* ---------------------------------------------------------- 事件绑定 */

function installEvents() {
  // 收起 / 展开
  el("btn-collapse").addEventListener("click", async () => {
    const collapsed = !(data?.settings?.window?.collapsed ?? false);
    $.widget.classList.toggle("collapsed", collapsed);
    $.body.classList.toggle("hidden", collapsed);
    $.collapsedBar.classList.toggle("hidden", !collapsed);
    await run(() => invoke("set_collapsed", { collapsed }));
    if (!collapsed) $.inputTitle.focus();
  });

  el("btn-min").addEventListener("click", () => invoke("hide_window").catch(() => {}));

  el("btn-add-quick").addEventListener("click", async () => {
    if (data?.settings?.window?.collapsed) {
      el("btn-collapse").click();
      setTimeout(() => $.inputTitle.focus(), 120);
    }
  });

  // 设置面板
  el("btn-settings").addEventListener("click", async () => {
    await saveSettings(false).catch(() => {});
    $.settings.classList.remove("hidden");
    try {
      await invoke("get_autostart").then((v) => (el("s-autostart").checked = !!v));
    } catch {}
    refreshGitStatus();
  });
  el("btn-settings-close").addEventListener("click", async () => {
    $.settings.classList.add("hidden");
    await saveSettings(false).catch(() => {});
  });
  el("settings-tabs").addEventListener("click", (e) => {
    const tab = e.target.closest(".tab");
    if (!tab) return;
    document.querySelectorAll(".tab").forEach((t) => t.classList.toggle("active", t === tab));
    document.querySelectorAll(".pane").forEach((p) =>
      p.classList.toggle("active", p.dataset.pane === tab.dataset.tab)
    );
  });

  // 过滤
  $.filterSeg.addEventListener("click", (e) => {
    const b = e.target.closest(".seg-btn");
    if (!b) return;
    filter = b.dataset.filter;
    $.filterSeg.querySelectorAll(".seg-btn").forEach((x) => x.classList.toggle("active", x === b));
    render();
  });

  // 新增
  const add = async () => {
    const title = $.inputTitle.value.trim();
    if (!title) {
      $.inputTitle.focus();
      return;
    }
    const due = fromDatetimeLocal($.inputDue.value);
    const payload = { title, notes: "", due_at: due, priority: newPriority, tags: [] };
    const ok = await run(() => invoke("add_todo", { payload }));
    if (ok) {
      $.inputTitle.value = "";
      $.inputDue.value = "";
      syncTimeField();
      $.inputTitle.focus();
    }
  };
  el("btn-add").addEventListener("click", add);
  $.inputTitle.addEventListener("keydown", (e) => {
    if (e.key === "Enter") add();
  });
  $.inputDue.addEventListener("keydown", (e) => {
    if (e.key === "Enter") add();
  });
  $.inputDue.addEventListener("change", syncTimeField);
  $.inputDue.addEventListener("input", syncTimeField);
  el("btn-clear-time").addEventListener("click", (e) => {
    e.preventDefault();
    $.inputDue.value = "";
    syncTimeField();
    $.inputTitle.focus();
  });
  // 点胶囊任意处都能唤起系统时间选择器
  el("time-field").addEventListener("click", (e) => {
    if (e.target === $.inputDue) return;
    try {
      $.inputDue.showPicker?.();
    } catch {}
    $.inputDue.focus();
  });
  $.prioGroup.addEventListener("click", (e) => {
    const c = e.target.closest(".chip");
    if (!c) return;
    newPriority = c.dataset.prio;
    $.prioGroup.querySelectorAll(".chip").forEach((x) => x.classList.toggle("active", x === c));
  });
  syncTimeField();

  // 设置项即时保存
  for (const id of ["s-reminder-enabled", "s-lead", "s-sound", "s-voice", "s-on-top", "s-theme", "s-start-hidden", "s-sort", "s-git-auto", "s-git-msg", "s-git-url", "s-git-remote", "s-git-branch", "s-git-name", "s-git-email", "s-git-user", "s-git-token", "s-git-ssh"]) {
    const node = el(id);
    node?.addEventListener("change", () => saveSettingsSoon());
  }
  el("s-autostart").addEventListener("change", async () => {
    try {
      const on = await invoke("set_autostart", { enabled: el("s-autostart").checked });
      el("s-autostart").checked = !!on;
      toast(on ? "已开启开机自启" : "已关闭开机自启");
    } catch (e) {
      toast(String(e), true);
    }
  });
  el("btn-test-notify").addEventListener("click", async () => {
    try {
      await invoke("notify_now", { title: "DeskTodo 测试通知", body: "能看到这条说明通知正常 ✅" });
      beep();
    } catch (e) {
      toast(String(e), true);
    }
  });
  el("btn-reset-window").addEventListener("click", async () => {
    try {
      await invoke("reposition_window");
      await reload();
      toast("窗口已移回主屏右上角");
    } catch (e) {
      toast(String(e), true);
    }
  });

  // Git
  el("btn-git-test").addEventListener("click", async () => {
    const s = collectSettings();
    setGitStatus("正在测试连接…", "");
    try {
      const msg = await invoke("git_test_remote", { sync: s.sync });
      setGitStatus(msg, "ok");
    } catch (e) {
      setGitStatus(String(e), "err");
    }
  });
  el("btn-git-init").addEventListener("click", async () => {
    await saveSettings(false).catch(() => {});
    setGitStatus("正在初始化…", "");
    try {
      const st = await invoke("git_init");
      setGitStatus(formatGitStatus(st), st.dirty ? "err" : "ok");
    } catch (e) {
      setGitStatus(String(e), "err");
    }
  });
  el("btn-git-sync").addEventListener("click", doGitSync);

  // 数据
  el("btn-export").addEventListener("click", async () => {
    try {
      const p = await invoke("export_data");
      if (p) toast("已导出到 " + p);
    } catch (e) {
      toast(String(e), true);
    }
  });
  el("btn-import-merge").addEventListener("click", () => doImport(true));
  el("btn-import-replace").addEventListener("click", () => doImport(false));
  el("btn-clear-done").addEventListener("click", async () => {
    const n = data.todos.filter((t) => t.done).length;
    if (!n) return toast("没有已完成的待办");
    if (!(await confirmBox(`清除 ${n} 项已完成的待办？`))) return;
    await run(() => invoke("clear_completed"));
  });
  el("btn-open-dir").addEventListener("click", async () => {
    const p = el("data-path").textContent;
    if (!p || p === "…") return toast("数据目录未知", true);
    if (openPath) await openPath(p).catch((e) => toast(String(e), true));
    else toast(p);
  });

  // 确认框
  el("confirm-yes").addEventListener("click", () => closeConfirm(true));
  el("confirm-no").addEventListener("click", () => closeConfirm(false));

  // 快捷键
  window.addEventListener("keydown", async (e) => {
    const mod = e.metaKey || e.ctrlKey;
    if (mod && e.key.toLowerCase() === "z" && !e.shiftKey) {
      const target = e.target;
      if (target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA")) return;
      e.preventDefault();
      await run(() => invoke("restore_todo"));
      toast("已撤销上一步删除");
    }
    if (mod && e.key.toLowerCase() === "n") {
      e.preventDefault();
      $.inputTitle.focus();
    }
    if (e.key === "Escape") {
      if (!$.settings.classList.contains("hidden")) {
        $.settings.classList.add("hidden");
        await saveSettings(false).catch(() => {});
      } else if (!$.confirm.classList.contains("hidden")) {
        closeConfirm(false);
      }
    }
  });
}

function setGitStatus(text, cls) {
  const node = el("git-status");
  node.textContent = text;
  node.className = "git-status" + (cls ? " " + cls : "");
}

function formatGitStatus(st) {
  if (!st) return "无状态";
  const lines = [];
  lines.push(st.isRepo ? `仓库：已初始化（分支 ${st.branch}）` : "仓库：尚未初始化");
  lines.push(st.configured ? `远程：${st.remoteUrl || "未填写"}` : "远程：未配置");
  if (st.lastCommit) lines.push(`最近提交：${st.lastCommit}${st.lastCommitAt ? " · " + relTime(st.lastCommitAt) : ""}`);
  if (st.lastSync) lines.push(`上次同步：${relTime(st.lastSync)}`);
  if (st.changedFiles?.length) lines.push(`待提交文件：${st.changedFiles.join(", ")}`);
  lines.push(`状态：${st.message}`);
  return lines.join("\n");
}

async function refreshGitStatus() {
  try {
    const st = await invoke("git_status");
    setGitStatus(formatGitStatus(st), st.dirty ? "" : "ok");
  } catch (e) {
    setGitStatus(String(e), "err");
  }
}

async function doGitSync() {
  await saveSettings(false).catch(() => {});
  setGitStatus("正在同步…", "");
  try {
    const res = await invoke("git_sync_now", { message: null });
    setGitStatus(formatGitStatus(res.status) + "\n\n" + res.message, "ok");
    toast("同步完成：" + res.message);
  } catch (e) {
    setGitStatus(String(e), "err");
    toast("同步失败", true);
  }
}

async function doImport(replace) {
  if (replace && !(await confirmBox("覆盖导入会替换本机全部待办数据，确定继续？"))) return;
  try {
    const res = await invoke("import_data", { merge: !replace });
    if (!res) return;
    await reload();
    toast(`导入完成：新增 ${res.added} 项，共 ${res.total} 项`);
  } catch (e) {
    toast(String(e), true);
  }
}

/* ---------------------------------------------------------- 后端事件 */

async function installBackendEvents() {
  await listen("due://fired", (ev) => {
    const list = ev.payload ?? [];
    trace(`due:fired count=${list.length}`);
    if (!list.length) return;
    const names = list.map((t) => t.title).filter(Boolean);
    beep();
    speak(`待办到期提醒：${names.join("，")}`);
    $.widget.classList.add("flash");
    setTimeout(() => $.widget.classList.remove("flash"), 3200);
    toast(`⏰ ${names[0] ?? "有事项到期"}${names.length > 1 ? ` 等 ${names.length} 项` : ""}`);
    refreshGitStatus?.();
  });

  await listen("ui://focus-input", () => {
    $.settings.classList.add("hidden");
    $.body.classList.remove("hidden");
    $.inputTitle.focus();
  });

  await listen("ui://sync-request", () => doGitSync());

  await listen("git://status", (ev) => {
    if (ev.payload?.message) setGitStatus(formatGitStatus(ev.payload), ev.payload.dirty ? "" : "ok");
  });
}

/* ---------------------------------------------------------- 启动 */

async function boot() {
  trace("boot:enter");
  installLogging();
  trace("boot:logging-installed");
  logInfo("boot: 开始初始化");
  logInfo(`boot: Tauri API = ${T ? Object.keys(T).join(",") : "缺失"}`);
  trace("boot:about-to-get-state");
  try {
    data = await invoke("get_state");
    trace(`boot:get-state-ok todos=${data.todos.length}`);
  } catch (e) {
    trace(`boot:get-state-FAIL ${e?.message ?? e}`);
    logError(`boot: get_state 失败 -> ${e?.message ?? e}`);
    throw e;
  }
  trace("boot:render-start");
  render();
  trace("boot:render-done");

  // 数据目录 / 版本信息
  el("data-path").textContent = data.data_dir || "系统应用数据目录 / com.desktodo.app";
  el("app-info").textContent =
    `DeskTodo v${data.version ?? "0.1.0"} · 数据格式 v${data.version ?? 1}` +
    ` · 存储文件 todos.json（原子写入 + 每小时备份）`;

  // 收起态
  const collapsed = data.settings.window.collapsed;
  $.widget.classList.toggle("collapsed", collapsed);
  $.body.classList.toggle("hidden", collapsed);
  $.collapsedBar.classList.toggle("hidden", !collapsed);

  installPointerDrag();
  installEvents();
  await installBackendEvents();

  // 通知权限（首次询问）
  try {
    const { isPermissionGranted, requestPermission } = T?.notification ?? {};
    if (isPermissionGranted && requestPermission) {
      let granted = await isPermissionGranted();
      if (!granted) granted = (await requestPermission()) === "granted";
      el("notify-perm-note").textContent = granted
        ? "系统通知权限：已授权"
        : "系统通知权限：未授权，请在系统设置中允许 DeskTodo 发送通知";
    }
  } catch {}

  if (!collapsed) $.inputTitle.focus();
  // 每 30 秒刷新一次相对时间显示
  setInterval(render, 30000);
}

function start() {
  trace("start:called");
  boot().catch((e) => {
    const msg = e?.message ?? String(e);
    trace(`start:boot-FAIL ${msg}`);
    logError(`启动失败：${msg}`);
    showFatal(msg);
  });
}

function showFatal(msg) {
  const pre = document.createElement("pre");
  pre.style.cssText =
    "padding:16px;color:#ff8a80;font:12px/1.5 ui-monospace,monospace;white-space:pre-wrap";
  pre.textContent = `启动失败：${msg}\n\n诊断信息见应用数据目录下的 boot-trace.log 与 frontend.log`;
  document.body.replaceChildren(pre);
}

// 脚本在 <head> 中以 module 方式加载，需等 DOM 就绪
if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", start, { once: true });
} else {
  start();
}
