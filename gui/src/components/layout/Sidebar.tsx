import { NavLink } from 'react-router-dom';
import { Database, Settings, Activity, Download } from 'lucide-react';

export default function Sidebar() {
  const linkClass = ({ isActive }: { isActive: boolean }) =>
    `flex items-center gap-3 px-4 py-3 rounded-lg transition-colors ${
      isActive
        ? 'bg-primary text-white'
        : 'text-gray-300 hover:bg-gray-700'
    }`;

  return (
    <aside className="w-64 bg-sidebar h-full flex flex-col p-4">
      <div className="mb-8">
        <h1 className="text-xl font-bold text-white">Switch API</h1>
        <p className="text-sm text-gray-400">配置管理工具</p>
      </div>

      <nav className="flex-1 space-y-2">
        <NavLink to="/profiles" className={linkClass}>
          <Database size={20} />
          <span>Profiles</span>
        </NavLink>
        <NavLink to="/config" className={linkClass}>
          <Settings size={20} />
          <span>Config</span>
        </NavLink>
        <NavLink to="/status" className={linkClass}>
          <Activity size={20} />
          <span>Status</span>
        </NavLink>
        <NavLink to="/export" className={linkClass}>
          <Download size={20} />
          <span>Export/Import</span>
        </NavLink>
      </nav>

      <div className="text-xs text-gray-500 mt-4">
        v0.1.0
      </div>
    </aside>
  );
}
