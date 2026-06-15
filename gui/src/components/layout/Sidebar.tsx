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
  return (
    <aside className="hidden h-full w-[212px] shrink-0 flex-col border-r border-line bg-surface md:flex">
      <div className="drag-region px-4 pb-4 pt-5">
        <div className="flex items-center gap-3">
          <div
            className="grid h-8 w-8 place-items-center rounded-md"
            style={{
              background: 'linear-gradient(180deg,#FB923C,#EA580C)',
              boxShadow: '0 1px 3px rgba(234,88,12,0.35), inset 0 1px 0 rgba(255,255,255,0.25)',
            }}
          >
            <svg viewBox="0 0 24 24" width="17" height="17" fill="none">
              <g stroke="#fff" strokeWidth="1.8" strokeLinecap="round">
                <line x1="12" y1="3" x2="12" y2="6" />
                <line x1="12" y1="18" x2="12" y2="21" />
                <line x1="3" y1="12" x2="6" y2="12" />
                <line x1="18" y1="12" x2="21" y2="12" />
                <line x1="5.6" y1="5.6" x2="7.7" y2="7.7" />
                <line x1="16.3" y1="16.3" x2="18.4" y2="18.4" />
                <line x1="18.4" y1="5.6" x2="16.3" y2="7.7" />
                <line x1="7.7" y1="16.3" x2="5.6" y2="18.4" />
              </g>
              <circle cx="12" cy="12" r="4" fill="#fff" />
            </svg>
          </div>
          <div className="leading-tight">
            <div className="text-[15px] font-bold tracking-tight text-ink">Helio</div>
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

      <div className="border-t border-line/70 px-4 py-3 font-mono text-[10.5px] text-ink-faint">v0.1.0</div>
    </aside>
  );
}
