import { StrictMode, useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { AdministratorsPanel, createSarmgAdminApplication, errorRequestId, useAdminApplication } from "@sarmg/admin-shell";
import { Button, Checkbox, ConfirmDangerDialog, Dialog, EmptyState, ErrorState, FormField, LoadingState, PageHeader, StatusBadge, TextField } from "@sarmg/admin-ui";
import "@sarmg/design-tokens/tokens.css";
import "@sarmg/design-tokens/tokens.dark.css";
import "@sarmg/design-tokens/reset.css";
import "@sarmg/design-tokens/accessibility.css";
import "@sarmg/web-fonts/fonts.css";
import "@sarmg/admin-ui/styles.css";
import "./styles.css";
import { administratorApi, isBackupUser, isOverview, isUndefined, request, type BackupUser, type Overview } from "./api";
import product from "../package.json";

type Failure = { requestId?: string };
type View = "overview" | "users" | "administrators";
const GIB = 1_073_741_824;
function currentView(): View {
  const hash = window.location.hash.slice(1);
  return hash === "users" || hash === "administrators" ? hash : "overview";
}
function quotaBytes(value: FormDataEntryValue | null): number {
  const text = String(value ?? "").trim(), result = Number(text) * GIB;
  if (text === "" || !Number.isSafeInteger(result) || result < 0) throw new Error("Invalid quota");
  return result;
}
function clearPassword(form: HTMLFormElement) {
  const field = form.elements.namedItem("password");
  if (field instanceof HTMLInputElement) { field.value = ""; field.focus(); }
}

function Application() {
  const [view, setView] = useState<View>(currentView);
  const [overview, setOverview] = useState<Overview | null>(null);
  const [failure, setFailure] = useState<Failure | null>(null);
  const [generation, setGeneration] = useState(0);
  const reload = () => setGeneration(value => value + 1);
  useEffect(() => {
    const changed = () => setView(currentView()); window.addEventListener("hashchange", changed);
    return () => window.removeEventListener("hashchange", changed);
  }, []);
  useEffect(() => {
    if (view === "administrators") return;
    const controller = new AbortController(); setOverview(null); setFailure(null);
    void request("/api/v2/admin/overview", isOverview, { signal: controller.signal })
      .then(value => { if (!controller.signal.aborted) setOverview(value); })
      .catch(error => { if (!controller.signal.aborted) setFailure({ requestId: errorRequestId(error) }); });
    return () => controller.abort();
  }, [generation, view]);
  return <div className="media-business">
    <PageHeader><h1>{view === "overview" ? "备份总览" : view === "users" ? "备份用户" : "平台管理员"}</h1>
      {view !== "administrators" && <Button onClick={reload}>刷新数据</Button>}</PageHeader>
    {view === "administrators" ? <AdministratorsPanel />
      : failure ? <ErrorState requestId={failure.requestId} onRetry={reload}>备份数据暂不可用，请重试。</ErrorState>
      : overview === null ? <LoadingState>正在载入备份数据…</LoadingState>
      : view === "overview" ? <OverviewView overview={overview} /> : <UsersView overview={overview} reload={reload} />}
  </div>;
}

function OverviewView({ overview }: { overview: Overview }) {
  return <div className="media-sections"><Section title="备份统计"><div className="media-grid">
    <Metric label="启用 / 全部用户" value={overview.active_users + " / " + overview.total_users} />
    <Metric label="媒体已用" value={bytes(overview.used_bytes)} />
    <Metric label="上传预留空间" value={bytes(overview.pending_bytes)} />
    <Metric label="已分配配额" value={bytes(overview.quota_bytes) + (overview.unlimited_users > 0 ? " + 不限" : "")} />
  </div></Section><Section title="用户概览"><div className="media-grid">
    {overview.users.length === 0 ? <EmptyState>暂无备份用户</EmptyState> : overview.users.map(user => <article className="media-card" key={user.id}>
      <h3>{user.display_name}</h3><StatusBadge status={user.enabled ? "已启用" : "已停用"} />
      <dl><dt>账号</dt><dd>{user.username}</dd><dt>设备</dt><dd>{user.device_count}</dd>
        <dt>资源</dt><dd>{user.resource_count}</dd><dt>容量</dt><dd>{bytes(user.used_bytes)} / {user.quota_bytes === 0 ? "不限" : bytes(user.quota_bytes)}</dd>
        <dt>上传预留</dt><dd>{bytes(user.pending_bytes)}</dd><dt>存储路径</dt><dd>{user.storage_path}</dd></dl>
      <progress max={1} value={user.quota_bytes > 0 ? Math.min(1, user.used_bytes / user.quota_bytes) : 0} aria-label={user.username + " 存储配额占用比例"} />
    </article>)}
  </div></Section></div>;
}

function UsersView({ overview, reload }: { overview: Overview; reload(): void }) {
  return <div className="media-sections"><p>备份用户用于设备上传，与平台管理员账户相互独立。配额为 0 表示不限。</p>
    <Section title="新增备份用户"><BackupUserForm reload={reload} /></Section>
    <Section title="管理备份用户"><div className="media-grid">
      {overview.users.length === 0 ? <EmptyState>暂无备份用户</EmptyState> : overview.users.map(user => <BackupUserForm key={user.id} user={user} reload={reload} />)}
    </div></Section></div>;
}

function BackupUserForm({ user, reload }: { user?: BackupUser; reload(): void }) {
  const { notify } = useAdminApplication();
  const busy = useRef(false);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<Failure | null>(null);
  const [password, setPassword] = useState(false);
  const [disableInput, setDisableInput] = useState<Record<string, unknown> | null>(null);
  async function save(input: Record<string, unknown>, form?: HTMLFormElement) {
    if (busy.current) return;
    busy.current = true; setPending(true); setFailure(null);
    try {
      await request(user ? "/api/v2/admin/users/" + user.id : "/api/v2/admin/users", isBackupUser,
        { method: user ? "PUT" : "POST", body: JSON.stringify(input) });
      form?.reset(); setDisableInput(null); notify(user ? "备份用户已保存" : "备份用户已创建"); reload();
    } catch (error) { setFailure({ requestId: errorRequestId(error) }); if (form) clearPassword(form); }
    finally { busy.current = false; setPending(false); }
  }
  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault(); if (busy.current) return;
    const form = event.currentTarget, data = new FormData(form); setFailure(null);
    try {
      const input = { username: String(data.get("username") ?? ""), display_name: String(data.get("display_name") ?? ""),
        storage_path: String(data.get("storage_path") ?? ""), quota_bytes: quotaBytes(data.get("quota_gib")),
        enabled: user ? data.has("enabled") : true, ...(user ? {} : { password: String(data.get("password") ?? "") }) };
      if (user?.enabled && !input.enabled) setDisableInput(input); else void save(input, form);
    } catch (error) { setFailure({ requestId: errorRequestId(error) }); clearPassword(form); }
  }
  const error = failure && <ErrorState requestId={failure.requestId}>未能保存备份用户，请检查账号、路径和配额后重试。</ErrorState>;
  return <article className="media-card">
    {user && <><h3>{user.display_name}</h3><StatusBadge status={user.enabled ? "已启用" : "已停用"} /></>}
    <form aria-label={user ? "编辑备份用户 " + user.username : "创建备份用户"} aria-busy={pending} onSubmit={submit}>
      {!disableInput && error}
      <FormField label="名称"><TextField name="display_name" defaultValue={user?.display_name ?? ""} required maxLength={100} readOnly={pending} /></FormField>
      <FormField label="账号"><TextField name="username" defaultValue={user?.username ?? ""} required minLength={3} maxLength={64} readOnly={pending} autoComplete="off" /></FormField>
      {!user && <FormField label="密码"><TextField name="password" type="password" required minLength={12} maxLength={128} readOnly={pending} autoComplete="new-password" /></FormField>}
      <FormField label="存储路径"><TextField name="storage_path" defaultValue={user?.storage_path ?? ""} placeholder="自动分配" required={!!user} readOnly={pending} /></FormField>
      <FormField label="配额（GiB，0 表示不限）"><TextField name="quota_gib" type="number" min={0} step="any" defaultValue={user ? user.quota_bytes / GIB : 100} required readOnly={pending} /></FormField>
      {user && <label className="media-check"><Checkbox name="enabled" defaultChecked={user.enabled} disabled={pending} />启用备份用户</label>}
      <div className="sarmg-actions"><Button type="submit" disabled={pending}>{pending ? "正在保存…" : user ? "保存备份用户" : "创建备份用户"}</Button>
        {user && <Button disabled={pending} onClick={() => setPassword(true)}>重设备份密码</Button>}</div>
    </form>
    {user && disableInput && <ConfirmDangerDialog title={"停用备份用户 " + user.username + "？"} description="该账户将无法继续上传，已有备份数据不会删除。" pending={pending}
      onClose={() => { if (!busy.current) { setDisableInput(null); setFailure(null); } }} onConfirm={() => void save(disableInput)}>{error}</ConfirmDangerDialog>}
    {user && password && <BackupPasswordDialog user={user} close={() => setPassword(false)} />}
  </article>;
}

function BackupPasswordDialog({ user, close }: { user: BackupUser; close(): void }) {
  const { notify } = useAdminApplication();
  const busy = useRef(false);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<Failure | null>(null);
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault(); if (busy.current) return;
    const form = event.currentTarget, data = new FormData(form);
    busy.current = true; setPending(true); setFailure(null);
    try {
      await request("/api/v2/admin/users/" + user.id + "/reset-password", isUndefined,
        { method: "POST", body: JSON.stringify({ password: String(data.get("password") ?? "") }) });
      form.reset(); notify("备份用户密码已重设"); close();
    } catch (error) { setFailure({ requestId: errorRequestId(error) }); clearPassword(form); }
    finally { busy.current = false; setPending(false); }
  }
  return <Dialog title={"重设备份密码 · " + user.username} onClose={() => { if (!busy.current) close(); }}>
    <form aria-busy={pending} onSubmit={event => void submit(event)}>
      {failure && <ErrorState requestId={failure.requestId}>未能重设密码，请重试。</ErrorState>}
      <FormField label="新备份密码"><TextField name="password" type="password" minLength={12} maxLength={128} required autoComplete="new-password" readOnly={pending} /></FormField>
      <div className="sarmg-actions"><Button disabled={pending} onClick={close}>取消</Button><Button type="submit" disabled={pending}>确认重设密码</Button></div>
    </form>
  </Dialog>;
}

function Metric({ label, value }: { label: string; value: string }) { return <article className="media-card"><h3>{label}</h3><p>{value}</p></article>; }
function Section({ title, children }: { title: string; children: ReactNode }) { return <section><h2>{title}</h2>{children}</section>; }
function bytes(value: number): string {
  for (const [scale, label] of [[2 ** 40, "TiB"], [2 ** 30, "GiB"], [2 ** 20, "MiB"], [2 ** 10, "KiB"]] as const) {
    if (value >= scale) return (value / scale).toFixed(1) + " " + label;
  }
  return value + " B";
}
const Root = createSarmgAdminApplication({ product: { name: "媒体备份管理中心", version: product.version }, client: administratorApi,
  navigation: [{ label: "总览", href: "#overview" }, { label: "备份用户", href: "#users" }, { label: "平台管理员", href: "#administrators" }], routes: <Application /> });
const root = document.getElementById("root");
if (root === null) throw new Error("缺少 React 根节点");
createRoot(root).render(<StrictMode><Root /></StrictMode>);
