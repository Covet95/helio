import { useEffect, useState } from 'react';
import { NavLink } from 'react-router-dom';
import { Layers, SlidersHorizontal, Activity, ArrowLeftRight, FileDown, History } from 'lucide-react';

const NAV = [
  { to: '/profiles', label: '配置档案', icon: Layers },
  { to: '/config', label: '共享配置', icon: SlidersHorizontal },
  { to: '/status', label: '状态', icon: Activity },
  { to: '/import', label: '从本地导入', icon: FileDown },
  { to: '/export', label: '备份 / 恢复', icon: ArrowLeftRight },
  { to: '/history', label: '会话历史', icon: History },
];

export default function Sidebar() {
  const [version, setVersion] = useState('0.1.1');

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
    <aside className="flex h-full w-[212px] shrink-0 flex-col border-r border-line bg-surface">
      <div className="drag-region px-4 pb-4 pt-5">
        <div className="flex min-h-10 items-center gap-3">
          <div
            className="grid h-10 w-10 shrink-0 place-items-center rounded-[10px]"
            style={{
              background: 'linear-gradient(180deg, #FF8A3D 0%, #F56817 100%)',
              boxShadow: '0 2px 5px rgba(234, 88, 12, 0.28), inset 0 1px 0 rgba(255, 255, 255, 0.24)',
            }}
            aria-hidden="true"
          >
            <svg className="block h-5 w-5" viewBox="0 0 24 24" fill="none">
              <circle cx="12" cy="12" r="3.25" stroke="white" strokeWidth="1.65" />
              <path
                d="M12 3.25V5.5M12 18.5v2.25M3.25 12H5.5M18.5 12h2.25M5.81 5.81 7.4 7.4m9.2 9.2 1.59 1.59m0-12.38L16.6 7.4m-9.2 9.2-1.59 1.59"
                stroke="white"
                strokeWidth="1.65"
                strokeLinecap="round"
              />
            </svg>
          </div>
          <div className="flex h-10 items-center">
            <div className="text-[15px] font-bold leading-none tracking-tight text-ink">Helio</div>
          </div>
        </div>
      </div>

      <nav className="flex-1 px-3 space-y-1">
        {NAV.map(({ to, label, icon: Icon }) => (
          <NavLink
            key={to}
            to={to}
            className={({ isActive }) =>
              `group relative flex items-center gap-2.5 rounded-md px-2.5 py-2 text-[13px] font-medium transition-all duration-150 ${
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
                <Icon size={17} strokeWidth={2} className={isActive ? 'text-accent' : ''} />
                <span>{label}</span>
              </>
            )}
          </NavLink>
        ))}
      </nav>

      <div className="border-t border-line/70 px-4 py-3 font-mono text-[10.5px] text-ink-faint">
        v{version}
      </div>
    </aside>
  );
}
