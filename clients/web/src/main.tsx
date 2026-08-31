import { StrictMode, useCallback, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import type { ChangeEvent, FormEvent, ReactNode } from "react";
import type { AdministratorSession } from "@sarmg/contracts";
import { useAdministratorSession } from "@sarmg/admin-web/react";
import { isApiClientError } from "@sarmg/http-client";

import "@sarmg/design-tokens/tokens.css";
import "@sarmg/design-tokens/reset.css";
import "@sarmg/design-tokens/accessibility.css";
import "./styles.css";

import {
  administratorApi,
  isBackupUser,
  isOverview,
  isUndefined,
  request,
  type BackupUser,
  type Overview,
} from "./api";

type View = "overview" | "users";
type Theme = "light" | "dark";
type UserDraft = Pick<BackupUser, "id" | "username" | "display_name" | "storage_path" | "quota_bytes" | "enabled"> & { password: string };

function Root() {
  const auth = useAdministratorSession(administratorApi);
  const [theme, setTheme] = useState<Theme>(() => readTheme());
  useEffect(() => {
    try { localStorage.setItem("media-backup-theme", theme); } catch { /* Preference storage is optional. */ }
  }, [theme]);

  if (auth.phase === "loading") return <main className="loading sarmg-theme" data-sarmg-theme={theme}>正在验证管理员会话…</main>;
  if (auth.phase !== "authenticated") return <Login theme={theme} login={auth.login} restoreFailed={auth.phase === "error"} />;
  return <Application session={auth.session} theme={theme} toggleTheme={() => setTheme(theme === "dark" ? "light" : "dark")} logout={auth.logout} />;
}

function Login({ theme, login, restoreFailed }: { theme: Theme; login(username: string, password: string): Promise<void>; restoreFailed: boolean }) {
  const [error, setError] = useState(restoreFailed ? "会话检查失败，请重新登录" : "");
  const [busy, setBusy] = useState(false);
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    setError(""); setBusy(true);
    try {
      await login(String(data.get("username") ?? ""), String(data.get("password") ?? ""));
      form.reset();
    } catch (caught) {
      setError(errorText(caught));
    } finally {
      setBusy(false);
    }
  };
  return <main className="app-shell sarmg-theme sarmg-login" data-sarmg-theme={theme}><form className="sarmg-card sarmg-login__card" aria-label="登录媒体备份管理中心" onSubmit={(event) => void submit(event)}><div className="sarmg-card__inner"><CardRow label="管理员用户名"><input name="username" className="sarmg-login__input" type="text" autoComplete="username" minLength={3} maxLength={64} required autoFocus /></CardRow><CardRow label="密码"><input name="password" className="sarmg-login__input" type="password" autoComplete="current-password" required /></CardRow><CardRow label="状态"><span className="sarmg-login__error" role="alert">{error}</span></CardRow><CardRow label="操作"><div className="sarmg-card__actions"><button className="sarmg-card__action sarmg-action-primary" type="submit" disabled={busy}>{busy ? "正在登录…" : "登录"}</button></div></CardRow></div></form></main>;
}

function Application({ session, theme, toggleTheme, logout }: { session: AdministratorSession; theme: Theme; toggleTheme(): void; logout(): Promise<void> }) {
  const [view, setView] = useState<View>("overview");
  const [overview, setOverview] = useState<Overview | null>(null);
  const [message, setMessage] = useState("");
  const load = useCallback(async () => setOverview(await request("/api/v2/admin/overview", isOverview)), []);
  useEffect(() => { void load().catch((error) => setMessage(errorText(error))); }, [load]);
  const notify = (next: string) => { setMessage(next); window.setTimeout(() => setMessage(""), 2_400); };

  return <div className="app-shell sarmg-theme" data-sarmg-theme={theme}><aside className="sidebar"><nav className="nav-list" aria-label="媒体备份导航"><button className={`nav-item ${view === "overview" ? "active" : ""}`} onClick={() => setView("overview")}>总览</button><button className={`nav-item ${view === "users" ? "active" : ""}`} onClick={() => setView("users")}>备份用户</button></nav><div className="sidebar-footer"><button className="icon-button" onClick={() => void load()} aria-label="刷新">↻</button><span className="connection-pill" role="status" title={session.username} aria-label={`管理员 ${session.username} 已连接`}>✓</span><button className="icon-button" onClick={toggleTheme} aria-label="切换主题">◐</button><button className="icon-button" onClick={() => void logout()} aria-label="退出登录">⏻</button></div></aside><main className="main">{overview === null ? <div className="empty-state">正在载入管理数据…</div> : view === "overview" ? <OverviewView overview={overview} /> : <UsersView overview={overview} reload={load} notify={notify} />}</main>{message !== "" && <div className="toast" role="status">{message}</div>}</div>;
}

function OverviewView({ overview }: { overview: Overview }) {
  return <section className="view-stack"><Section title="备份"><div className="sarmg-grid"><Metric label="用户" value={`${overview.active_users} / ${overview.total_users}`} detail="启用 / 全部" /><Metric label="已用" value={bytes(overview.used_bytes)} detail="媒体数据" /><Metric label="上传" value={bytes(overview.pending_bytes)} detail="预留空间" /><Metric label="配额" value={`${bytes(overview.quota_bytes)}${overview.unlimited_users > 0 ? " + 不限" : ""}`} detail="已分配" /></div></Section><Section title="用户概览"><div className="sarmg-grid">{overview.users.length === 0 ? <div className="empty-state">暂无用户</div> : overview.users.map((user) => <SummaryCard key={user.id} user={user} />)}</div></Section></section>;
}

function UsersView({ overview, reload, notify }: { overview: Overview; reload(): Promise<void>; notify(message: string): void }) {
  const create = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault(); const form = event.currentTarget; const data = new FormData(form);
    await request("/api/v2/admin/users", isBackupUser, { method: "POST", body: JSON.stringify({ username: String(data.get("username") ?? ""), password: String(data.get("password") ?? ""), display_name: String(data.get("display_name") ?? ""), storage_path: String(data.get("storage_path") ?? ""), quota_bytes: Math.round(Number(data.get("quota_gb")) * 1_073_741_824), enabled: true }) });
    form.reset(); notify("备份用户已创建"); await reload();
  };
  return <section className="view-stack"><Section title="新增备份用户"><div className="sarmg-grid"><form className="sarmg-card" onSubmit={(event) => void create(event).catch((error) => notify(errorText(error)))}><div className="sarmg-card__inner"><CardRow label="账号"><input name="username" className="card-input" minLength={3} maxLength={64} required /></CardRow><CardRow label="密码"><input name="password" className="card-input" type="password" minLength={12} maxLength={128} required /></CardRow><CardRow label="名称"><input name="display_name" className="card-input" maxLength={100} required /></CardRow><CardRow label="路径"><input name="storage_path" className="card-input" placeholder="自动分配" /></CardRow><CardRow label="配额"><input name="quota_gb" className="card-input" type="number" min={0} step={1} defaultValue={100} required /><span>GB</span></CardRow><CardRow label="操作"><button className="sarmg-card__action sarmg-action-primary" type="submit">创建</button></CardRow></div></form></div></Section><Section title="备份用户"><div className="sarmg-grid">{overview.users.map((user) => <UserEditor key={user.id} user={user} reload={reload} notify={notify} />)}</div></Section></section>;
}

function UserEditor({ user, reload, notify }: { user: BackupUser; reload(): Promise<void>; notify(message: string): void }) {
  const [draft, setDraft] = useState<UserDraft>({ id: user.id, username: user.username, display_name: user.display_name, storage_path: user.storage_path, quota_bytes: user.quota_bytes, enabled: user.enabled, password: "" });
  const save = async () => {
    await request(`/api/v2/admin/users/${draft.id}`, isBackupUser, { method: "PUT", body: JSON.stringify({ username: draft.username, display_name: draft.display_name, storage_path: draft.storage_path, quota_bytes: draft.quota_bytes, enabled: draft.enabled }) });
    if (draft.password !== "") await request(`/api/v2/admin/users/${draft.id}/reset-password`, isUndefined, { method: "POST", body: JSON.stringify({ password: draft.password }) });
    setDraft({ ...draft, password: "" }); notify("备份用户已保存"); await reload();
  };
  const text = (key: "username" | "display_name" | "storage_path" | "password") => (event: ChangeEvent<HTMLInputElement>) => setDraft({ ...draft, [key]: event.target.value });
  return <article className="sarmg-card"><div className="sarmg-card__inner"><CardRow label="名称"><input className="card-input" value={draft.display_name} onChange={text("display_name")} /><i className={`sarmg-status-led ${draft.enabled ? "sarmg-status-good" : "sarmg-status-danger"}`} /></CardRow><CardRow label="账号"><input className="card-input" value={draft.username} onChange={text("username")} /></CardRow><CardRow label="路径"><input className="card-input" value={draft.storage_path} onChange={text("storage_path")} /></CardRow><CardRow label="配额"><input className="card-input" type="number" min={0} step={0.1} value={gigabytes(draft.quota_bytes)} onChange={(event) => setDraft({ ...draft, quota_bytes: Math.round(Number(event.target.value) * 1_073_741_824) })} /><span>GB</span></CardRow><CardRow label="密码"><input className="card-input" type="password" minLength={12} maxLength={128} value={draft.password} onChange={text("password")} placeholder="不修改" /></CardRow><CardRow label="操作"><div className="sarmg-card__actions"><button className="sarmg-card__action sarmg-action-primary" type="button" onClick={() => void save().catch((error) => notify(errorText(error)))}>保存</button><label className="card-check"><input type="checkbox" checked={draft.enabled} onChange={(event) => setDraft({ ...draft, enabled: event.target.checked })} />启用</label></div></CardRow></div></article>;
}

function SummaryCard({ user }: { user: BackupUser }) { const ratio = user.quota_bytes > 0 ? Math.min(1, user.used_bytes / user.quota_bytes) : 0; return <article className="sarmg-card"><div className="sarmg-card__inner"><CardRow label="名称"><span className="sarmg-truncate sarmg-grow">{user.display_name}</span><i className={`sarmg-status-led ${user.enabled ? "sarmg-status-good" : "sarmg-status-danger"}`} /></CardRow><CardRow label="账号"><span className="sarmg-truncate">{user.username}</span></CardRow><CardRow label="设备">{user.device_count}</CardRow><CardRow label="资源">{user.resource_count}</CardRow><CardRow label="占用"><progress max={1} value={ratio} aria-label="存储配额占用比例" /></CardRow><CardRow label="容量"><span className="sarmg-truncate sarmg-muted">{bytes(user.used_bytes)} / {user.quota_bytes === 0 ? "不限" : bytes(user.quota_bytes)}</span></CardRow></div></article>; }
function Metric({ label, value, detail }: { label: string; value: string; detail: string }) { return <article className="sarmg-card"><div className="sarmg-card__inner"><CardRow label={label}><strong className="metric-row-value">{value}</strong></CardRow><CardRow label="详情"><span className="metric-row-detail">{detail}</span></CardRow></div></article>; }
function Section({ title, children }: { title: string; children: ReactNode }) { return <section className="section-band"><header className="section-header"><div className="section-title"><span aria-hidden="true">◇</span><h2>{title}</h2></div></header>{children}</section>; }
function CardRow({ label, children }: { label: string; children: ReactNode }) { return <div className="sarmg-card__row"><span className="sarmg-card__label">{label}</span><div className="sarmg-card__content">{children}</div></div>; }

function readTheme(): Theme { try { return localStorage.getItem("media-backup-theme") === "dark" ? "dark" : "light"; } catch { return "light"; } }
function bytes(value: number): string { if (value >= 1_099_511_627_776) return `${(value / 1_099_511_627_776).toFixed(1)} TB`; if (value >= 1_073_741_824) return `${(value / 1_073_741_824).toFixed(1)} GB`; if (value >= 1_048_576) return `${(value / 1_048_576).toFixed(1)} MB`; if (value >= 1_024) return `${(value / 1_024).toFixed(1)} KB`; return `${value} B`; }
const gigabytes = (value: number) => value === 0 ? 0 : Math.round((value / 1_073_741_824) * 100) / 100;
const errorText = (error: unknown) => isApiClientError(error)
  ? error.message
  : error instanceof Error ? error.message : "请求失败";

const root = document.getElementById("root");
if (root === null) throw new Error("缺少React根节点");
createRoot(root).render(<StrictMode><Root /></StrictMode>);
