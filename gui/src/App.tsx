import { HashRouter, Routes, Route, Navigate } from 'react-router-dom';
import { useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { useStore } from './store';
import Sidebar from './components/layout/Sidebar';
import ProfilesPage from './pages/ProfilesPage';
import ConfigPage from './pages/ConfigPage';
import StatusPage from './pages/StatusPage';
import ExportPage from './pages/ExportPage';
import ImportPage from './pages/ImportPage';
import HistoryPage from './pages/HistoryPage';

function App() {
  const { fetchProfiles, fetchStatus, lastError, clearError } = useStore();

  useEffect(() => {
    fetchProfiles();
    fetchStatus();
  }, [fetchProfiles, fetchStatus]);

  // 状态栏切换 profile 后，后端 emit "profile-switched"，刷新当前状态与列表
  useEffect(() => {
    const unlistenPromise = listen('profile-switched', () => {
      fetchStatus();
      fetchProfiles();
    });
    return () => {
      unlistenPromise.then((unlisten) => unlisten());
    };
  }, [fetchStatus, fetchProfiles]);

  return (
    <HashRouter>
      <div className="app-bg w-full h-full flex text-ink">
        <Sidebar />
        <main className="flex-1 min-w-0 overflow-y-auto overflow-x-hidden">
          {lastError && (
            <div className="sticky top-0 z-20 border-b border-danger/30 bg-danger/10 px-4 py-2 text-[12.5px] text-danger sm:px-7">
              <div className="flex items-start justify-between gap-3">
                <span>{lastError}</span>
                <button type="button" className="shrink-0 underline" onClick={clearError}>关闭</button>
              </div>
            </div>
          )}
          <Routes>
            <Route path="/" element={<Navigate to="/profiles" replace />} />
            <Route path="/profiles" element={<ProfilesPage />} />
            <Route path="/config" element={<ConfigPage />} />
            <Route path="/status" element={<StatusPage />} />
            <Route path="/import" element={<ImportPage />} />
            <Route path="/export" element={<ExportPage />} />
            <Route path="/history" element={<HistoryPage />} />
          </Routes>
        </main>
      </div>
    </HashRouter>
  );
}

export default App;
