import { useEffect, useState } from 'react';
import { NavLink } from 'react-router-dom';
import { Layers, SlidersHorizontal, Activity, ArrowLeftRight, FileDown, History, Sun, PanelLeftClose, PanelLeftOpen } from 'lucide-react';
import { useStore } from '../../store';

const NAV = [
  { to: '/profiles', label: '配置档案', icon: Layers },
  { to: '/config', label: '共享配置', icon: SlidersHorizontal },
  { to: '/status', label: '状态', icon: Activity },
  { to: '/import', label: '从本地导入', icon: FileDown },
  { to: '/export', label: '备份 / 恢复', icon: ArrowLeftRight },
  { to: '/history', label: '会话历史', icon: History },
];

export default function Sidebar() {
  const [version, setVersion] = useState('0.2.0');
  const collapsed = useStore((state) => state.sidebarCollapsed);
  const toggleSidebar = useStore((state) => state.toggleSidebar);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const { getVersion } = await import('@tauri-apps/api/app');
        const v = await getVersion();
        if (!cancelled && v) setVersion(v);
      } catch {
        // browser / non-tauri: keep package default
      }
    })();
    return () => { cancelled = true; };
  }, []);

  return (
    <aside className={`flex h-full w-16 shrink-0 flex-col border-r border-line bg-card ${collapsed ? '' : 'md:w-[200px]'}`}>
      <div className="drag-region px-3 pb-5 pt-5">
        <div className="flex min-h-10 items-center gap-2.5">
          <div
            className="grid h-10 w-10 shrink-0 place-items-center rounded-lg"
            style={{
              background: 'linear-gradient(180deg, #FF8A3D 0%, #F56817 100%)',
              boxShadow: '0 2px 5px rgba(234, 88, 12, 0.28), inset 0 1px 0 rgba(255, 255, 255, 0.24)',
            }}
            aria-hidden="true"
          >
            <Sun size={22} strokeWidth={1.8} className="text-white" />
          </div>
          <div className={`${collapsed ? 'hidden' : 'hidden md:flex'} h-10 items-center`}>
            <div className="text-[20px] font-bold leading-none text-ink">Helio</div>
          </div>
        </div>
      </div>

      <nav aria-label="主导航" className="min-h-0 flex-1 space-y-1 overflow-y-auto px-2">
        {NAV.map(({ to, label, icon: Icon }) => (
          <NavLink
            key={to}
            to={to}
            title={label}
            aria-label={label}
            className={({ isActive }) =>
              `group relative flex min-h-11 items-center justify-center gap-2.5 rounded-md px-2.5 py-2 text-[13px] font-medium transition-colors duration-150 ${collapsed ? '' : 'md:justify-start'} ${
                isActive
                  ? 'text-ink bg-elevated'
                  : 'text-ink-dim hover:text-ink hover:bg-elevated/60'
              }`
            }
          >
            {({ isActive }) => (
              <>
                <span
                  className={`absolute left-0 top-1/2 -translate-y-1/2 h-5 w-[3px] rounded-full bg-accent transition-all duration-300 ${
                    isActive ? 'opacity-100 scale-y-100' : 'opacity-0 scale-y-0'
                  }`}
                />
                <Icon size={18} strokeWidth={2} className={`shrink-0 ${isActive ? 'text-accent' : ''}`} />
                <span className={collapsed ? 'hidden' : 'hidden md:inline'}>{label}</span>
              </>
            )}
          </NavLink>
        ))}
      </nav>

      <div className="flex flex-wrap items-center justify-between gap-2 border-t border-line px-3 py-3">
        <span className={`${collapsed ? 'hidden' : 'hidden md:inline'} font-mono text-[11px] text-ink-faint`}>v{version}</span>
        <button type="button" onClick={toggleSidebar} title={collapsed ? '展开导航' : '收起导航'}
          aria-label={collapsed ? '展开导航' : '收起导航'} aria-expanded={!collapsed} className="icon-button hidden md:grid">
          {collapsed ? <PanelLeftOpen size={17} /> : <PanelLeftClose size={17} />}
        </button>
      </div>
    </aside>
  );
}
