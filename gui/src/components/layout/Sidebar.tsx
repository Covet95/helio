import { NavLink } from 'react-router-dom';
import { Layers, SlidersHorizontal, Activity, ArrowLeftRight } from 'lucide-react';

const NAV = [
  { to: '/profiles', label: 'Profiles', icon: Layers },
  { to: '/config', label: 'Shared Config', icon: SlidersHorizontal },
  { to: '/status', label: 'Status', icon: Activity },
  { to: '/export', label: 'Import / Export', icon: ArrowLeftRight },
];

export default function Sidebar() {
  return (
    <aside className="w-[232px] shrink-0 h-full flex flex-col border-r border-line bg-surface/60 backdrop-blur-xl">
      {/* brand / titlebar (draggable) */}
      <div className="drag-region px-5 pt-6 pb-5">
        <div className="flex items-center gap-3">
          <div className="relative grid place-items-center h-9 w-9 rounded-lg bg-gradient-to-br from-accent to-opacity-90 shadow-[0_4px_16px_-4px_rgb(59_130_246/0.6)]"
               style={{ background: 'linear-gradient(135deg,#3B82F6,#6366F1)' }}>
            <ArrowLeftRight size={18} className="text-white" strokeWidth={2.5} />
          </div>
          <div className="leading-tight">
            <div className="font-mono text-[15px] font-bold tracking-tight text-ink">switch<span className="text-accent">·</span>api</div>
            <div className="text-[11px] text-ink-faint tracking-wide">provider core</div>
          </div>
        </div>
      </div>

      {/* nav */}
      <nav className="flex-1 px-3 space-y-1">
        {NAV.map(({ to, label, icon: Icon }) => (
          <NavLink
            key={to}
            to={to}
            className={({ isActive }) =>
              `group relative flex items-center gap-3 px-3 py-2.5 rounded-lg text-[13.5px] font-medium transition-all duration-200 ${
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

      {/* footer */}
      <div className="px-5 py-4 border-t border-line/70">
        <div className="flex items-center gap-2 text-[11px] text-ink-faint">
          <span className="h-1.5 w-1.5 rounded-full bg-ok animate-breathe" />
          <span className="font-mono">v0.1.0 · 4 tools</span>
        </div>
      </div>
    </aside>
  );
}
