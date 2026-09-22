import { useState } from 'react';
import { Alert } from '@/components/common/Alert';
import { Button } from '../components/common/Button';
import { PageHeader } from '../components/common/PageHeader';
import { Download, Upload, FolderCog, RotateCcw, ChevronDown } from 'lucide-react';
import { ConfirmDialog } from '../components/common/Modal';
import { tauriApi, type DatabaseBackupInfo } from '../lib/tauri';
import { useStore } from '../store';
import { cn, formatBackupTime, formatBytes, humanizeError } from '../lib/utils';
import { tauriTransferDeps } from './exportDialog';
import { runTransfer, type TransferSpec } from './exportFlow';
import {
  databaseExportMessage,
  databaseImportMessage,
  portableExportMessage,
  portableImportMessage,
  skillsExportMessage,
  skillsImportMessage,
  type Feedback,
} from './exportMessages';

const DB_FILTER = { filterName: 'Database', extensions: ['db', 'sqlite'] };
const ARCHIVE_FILTER = { filterName: 'Helio 便携备份', extensions: ['tar.gz', 'tgz'] };
const SKILLS_FILTER = { filterName: 'Skills 备份', extensions: ['tar.gz', 'tgz'] };

export default function ExportPage() {
  const [portableImporting, setPortableImporting] = useState(false);
  const [portableExporting, setPortableExporting] = useState(false);
  const [importing, setImporting] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [skillsImporting, setSkillsImporting] = useState(false);
  const [skillsExporting, setSkillsExporting] = useState(false);
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const [confirmPortableImport, setConfirmPortableImport] = useState(false);
  const [confirmImport, setConfirmImport] = useState(false);
  const [confirmSkillsImport, setConfirmSkillsImport] = useState(false);
  /**
   * 是否有任一传输在进行中。
   *
   * 各按钮的 `xxxing` 只禁用自己——便携备份写到一半时，数据库导出按钮仍可点，
   * 于是两个导出并发跑。它们争的是同一把后端写锁，不会损坏数据，但用户看到
   * 两个进度同时转、两条反馈互相覆盖，分不清哪个结果对应哪次操作。
   * 用这个总开关把整组按钮一起锁上。
   */
  const busy =
    portableExporting || portableImporting || exporting || importing || skillsExporting || skillsImporting;

  const refreshAppData = async () => {
    try {
      await useStore.getState().fetchProfiles();
      await useStore.getState().fetchStatus();
    } catch {
      window.location.reload();
    }
  };

  /**
   * 统一走 `runTransfer`：置忙 → 清反馈 → 执行 → 写反馈 → 复位。
   *
   * `afterSuccess` 在反馈**之后**跑——导入的提示语是「正在刷新…」，
   * 得先让它显示出来再刷新。
   */
  const transfer = async (
    spec: TransferSpec,
    setBusy: (v: boolean) => void,
    afterSuccess?: () => Promise<void>,
  ) => {
    setBusy(true);
    setFeedback(null);
    try {
      const outcome = await runTransfer(tauriTransferDeps(), spec);
      setFeedback(outcome);
      if (outcome.kind === 'success' && afterSuccess) await afterSuccess();
    } finally {
      setBusy(false);
    }
  };

  const handleExport = () =>
    transfer(
      {
        mode: 'export',
        action: '导出',
        request: { ...DB_FILTER, defaultPath: `helio-backup-${Date.now()}.db` },
        execute: async (path) => {
          await tauriApi.exportDatabase(path);
          return { text: databaseExportMessage(path), kind: 'success' };
        },
      },
      setExporting,
    );

  const handlePortableExport = () =>
    transfer(
      {
        mode: 'export',
        action: '便携备份导出',
        request: { ...ARCHIVE_FILTER, defaultPath: `helio-portable-${Date.now()}.tar.gz` },
        execute: async (path) => {
          const result = await tauriApi.exportPortableBackup(path);
          // 用后端回报的路径（而非用户选的路径）：跨设备/软链时二者可能不同。
          return { text: portableExportMessage(result.skills.total, result.path), kind: 'success' };
        },
      },
      setPortableExporting,
    );

  const handlePortableImport = () => {
    setConfirmPortableImport(false);
    return transfer(
      {
        mode: 'import',
        action: '便携备份恢复',
        request: ARCHIVE_FILTER,
        execute: async (path) => {
          const result = await tauriApi.importPortableBackup(path);
          return { text: portableImportMessage(result), kind: 'success' };
        },
      },
      setPortableImporting,
      refreshAppData,
    );
  };

  const handleImport = () => {
    setConfirmImport(false);
    return transfer(
      {
        mode: 'import',
        action: '导入',
        request: DB_FILTER,
        execute: async (path) => {
          await tauriApi.importDatabase(path);
          return { text: databaseImportMessage(), kind: 'success' };
        },
      },
      setImporting,
      refreshAppData,
    );
  };

  const handleSkillsExport = () =>
    transfer(
      {
        mode: 'export',
        action: 'Skills 导出',
        request: { ...SKILLS_FILTER, defaultPath: `helio-skills-${Date.now()}.tar.gz` },
        execute: async (path) => skillsExportMessage(await tauriApi.exportSkills(path)),
      },
      setSkillsExporting,
    );

  const handleSkillsImport = () => {
    setConfirmSkillsImport(false);
    return transfer(
      {
        mode: 'import',
        action: 'Skills 导入',
        request: SKILLS_FILTER,
        execute: async (path) => {
          const result = await tauriApi.importSkills(path);
          return { text: skillsImportMessage(result), kind: 'success' };
        },
      },
      setSkillsImporting,
    );
  };

  return (
    <div className="min-h-full">
      <PageHeader title="备份 / 恢复" />

      <div className="max-w-3xl px-4 py-4 sm:px-7 sm:py-5">
        {feedback && (
          <Alert
            tone={feedback.kind === 'success' ? 'success' : feedback.kind === 'error' ? 'error' : 'info'}
            className="mb-4"
          >
            {feedback.text}
          </Alert>
        )}

        <div className="overflow-hidden rounded-lg border border-line bg-card">
          <ActionRow
            icon={<Download size={20} className="text-accent" />}
            title="导出便携备份"
            meta="推荐 · 数据库 + Skills，换机迁移用这个"
            button={<Button onClick={handlePortableExport} disabled={busy}><Download size={16} />{portableExporting ? '导出中…' : '导出'}</Button>}
          />
          <ActionRow
            icon={<Upload size={20} className="text-opencode" />}
            title="恢复便携备份"
            meta="推荐 · 校验后恢复数据库、Skills 与激活配置"
            button={<Button variant="secondary" onClick={() => setConfirmPortableImport(true)} disabled={busy}><Upload size={16} />{portableImporting ? '恢复中…' : '恢复'}</Button>}
          />
        </div>
        <p className="mt-3 text-[12px] leading-relaxed text-ink-faint">
          单个配置文件的版本回退不在这里：在「共享配置」页底部按工具查看自动备份并恢复。
        </p>

        <details className="mt-4 overflow-hidden rounded-lg border border-line bg-card">
          <summary className="cursor-pointer px-4 py-3 text-[13px] font-medium text-ink-dim hover:text-ink">
            高级：单独备份数据库 / Skills
            <span className="mt-0.5 block text-[11px] font-normal text-ink-faint">便携备份已包含这两项；一般不需要单独操作</span>
          </summary>
          <div className="border-t border-line">
            <ActionRow
              icon={<Download size={20} className="text-accent" />}
              title="导出数据库"
              meta=".db / .sqlite"
              button={<Button onClick={handleExport} disabled={busy}><Download size={16} />{exporting ? '导出中…' : '导出'}</Button>}
            />
            <ActionRow
              icon={<Upload size={20} className="text-opencode" />}
              title="导入数据库"
              meta="仅接受 Helio 备份 · 覆盖前自动备份"
              button={<Button variant="secondary" onClick={() => setConfirmImport(true)} disabled={busy}><Upload size={16} />{importing ? '导入中…' : '导入'}</Button>}
            />
            <ActionRow
              icon={<FolderCog size={20} className="text-accent" />}
              title="导出 Skills"
              meta="全部工具 Skills 目录"
              button={<Button onClick={handleSkillsExport} disabled={busy}><Download size={16} />{skillsExporting ? '导出中…' : '导出'}</Button>}
            />
            <ActionRow
              icon={<FolderCog size={20} className="text-opencode" />}
              title="导入 Skills"
              meta="tar.gz · 整体校验 · 同名跳过"
              button={<Button variant="secondary" onClick={() => setConfirmSkillsImport(true)} disabled={busy}><Upload size={16} />{skillsImporting ? '导入中…' : '导入'}</Button>}
            />
            <DbBackupList disabled={busy} onRestored={refreshAppData} />
          </div>
        </details>
        </div>

        {confirmPortableImport && (
          <ConfirmDialog
            title="恢复便携备份"
            message="当前数据库会被覆盖。归档校验通过后会恢复 Skills，并把导入档案中已激活的工具配置写回本机。目标端同名 Skill 不会覆盖。"
            confirmText="恢复"
            danger
            onCancel={() => setConfirmPortableImport(false)}
            onConfirm={handlePortableImport}
          />
        )}

        {confirmImport && (
        <ConfirmDialog
          title="导入数据库"
          message="当前数据库会被覆盖。仅接受 Helio 导出的备份文件，校验不通过则不会改动现有数据。导入前自动备份当前库（带时间戳，最多保留 10 份，可回退）。"
          confirmText="导入"
          danger
          onCancel={() => setConfirmImport(false)}
          onConfirm={handleImport}
        />
      )}

      {confirmSkillsImport && (
        <ConfirmDialog
          title="导入 Skills"
          message="将从备份恢复到各工具对应目录。归档会先整体校验（拒绝路径穿越、异常条目与超大文件），校验不通过不写入任何文件；本地已存在的同名 Skill 会跳过、不会覆盖。"
          confirmText="导入"
          onCancel={() => setConfirmSkillsImport(false)}
          onConfirm={handleSkillsImport}
        />
      )}
    </div>
  );
}

/**
 * 数据库自动备份的回退入口。
 *
 * 导入确认框一直承诺「导入前自动备份当前库，可回退」，但此前**没有任何地方
 * 能看到这些备份**——承诺是空的。这里补上：列出备份、可一键回退。
 */
function DbBackupList({
  disabled,
  onRestored,
}: {
  disabled: boolean;
  onRestored: () => Promise<void>;
}) {
  const [open, setOpen] = useState(false);
  const [backups, setBackups] = useState<DatabaseBackupInfo[] | null>(null);
  const [restoring, setRestoring] = useState<string | null>(null);
  const [error, setError] = useState('');
  const [pending, setPending] = useState<DatabaseBackupInfo | null>(null);

  const load = async () => {
    setError('');
    try {
      setBackups(await tauriApi.listDatabaseBackups());
    } catch (e) {
      setError(humanizeError(e));
      setBackups([]);
    }
  };

  const toggle = () => {
    const next = !open;
    setOpen(next);
    if (next) void load();
  };

  const doRestore = async (backup: DatabaseBackupInfo) => {
    setPending(null);
    setRestoring(backup.path);
    setError('');
    try {
      await tauriApi.restoreDatabaseBackup(backup.path);
      await Promise.all([load(), onRestored()]);
    } catch (e) {
      setError(humanizeError(e));
    } finally {
      setRestoring(null);
    }
  };

  return (
    <>
      <div className="border-b border-line last:border-b-0">
        <button
          type="button"
          onClick={toggle}
          aria-expanded={open}
          disabled={disabled}
          className="flex w-full items-center gap-3 px-4 py-3.5 text-left transition-colors hover:bg-elevated/45 disabled:opacity-40"
        >
          <div className="grid h-9 w-9 shrink-0 place-items-center rounded-md border border-line bg-surface">
            <RotateCcw size={20} className="text-warn" />
          </div>
          <div className="min-w-0 flex-1">
            <h3 className="text-[14px] font-semibold text-ink">回退数据库</h3>
            <p className="truncate text-[12px] text-ink-faint">
              导入与迁移前的自动备份，最多保留 10 份
            </p>
          </div>
          <ChevronDown size={16} className={cn('shrink-0 text-ink-faint transition-transform', open && 'rotate-180')} />
        </button>

        {open && (
          <div className="border-t border-line/60 bg-surface/40 px-4 py-3">
            {error && <Alert tone="error" className="mb-2">{error}</Alert>}
            {backups === null ? (
              <div className="py-3 text-center text-[12px] text-ink-faint">读取中…</div>
            ) : backups.length === 0 ? (
              <div className="py-3 text-center text-[12px] text-ink-faint">
                还没有自动备份（导入或迁移前会自动生成）
              </div>
            ) : (
              <ul className="space-y-1.5">
                {backups.map((b) => (
                  <li key={b.path} className="flex flex-wrap items-center gap-2 rounded-md border border-line bg-card px-3 py-2">
                    <span className="shrink-0 font-mono text-[11.5px] text-ink-dim">{formatBackupTime(b.time)}</span>
                    <span className="shrink-0 text-[11px] text-ink-faint">{formatBytes(b.size_bytes)}</span>
                    <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-ink-faint" title={b.path}>
                      {b.path}
                    </span>
                    <Button
                      size="sm"
                      variant="secondary"
                      disabled={restoring !== null || disabled}
                      onClick={() => setPending(b)}
                    >
                      <RotateCcw size={13} className={restoring === b.path ? 'animate-spin' : ''} />
                      {restoring === b.path ? '回退中…' : '回退'}
                    </Button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}
      </div>

      {pending && (
        <ConfirmDialog
          title="回退数据库"
          message={
            `将用这份备份覆盖当前数据库：\n${formatBackupTime(pending.time)}\n\n` +
            `回退前会再自动备份一次当前库，仍可再退回。`
          }
          confirmText="回退"
          danger
          onCancel={() => setPending(null)}
          onConfirm={() => doRestore(pending)}
        />
      )}
    </>
  );
}

function ActionRow({ icon, title, meta, button }: {
  icon: React.ReactNode; title: string; meta: string; button: React.ReactNode;
}) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-4 border-b border-line px-4 py-3.5 last:border-b-0">
      <div className="flex min-w-0 items-center gap-3">
        <div className="grid h-9 w-9 shrink-0 place-items-center rounded-md border border-line bg-surface">{icon}</div>
        <div className="min-w-0">
          <h3 className="truncate text-[14px] font-semibold text-ink">{title}</h3>
          <p className="truncate text-[12px] text-ink-faint">{meta}</p>
        </div>
      </div>
      {button}
    </div>
  );
}
