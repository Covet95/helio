import { HashRouter, Routes, Route, Navigate } from 'react-router-dom';
import { lazy, Suspense, useEffect } from 'react';
import { isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { X } from 'lucide-react';
import { useShallow } from 'zustand/react/shallow';
import { useStore } from './store';
import Sidebar from './components/layout/Sidebar';
import ProfilesPage from './pages/ProfilesPage';
import { Spinner } from './components/common/Spinner';

const ConfigPage = lazy(() => import('./pages/ConfigPage'));
const StatusPage = lazy(() => import('./pages/StatusPage'));
const ExportPage = lazy(() => import('./pages/ExportPage'));
const ImportPage = lazy(() => import('./pages/ImportPage'));
const HistoryPage = lazy(() => import('./pages/HistoryPage'));

function App() {
  const { fetchProfiles, fetchStatus, refresh, lastError, clearError } = useStore(useShallow((state) => ({
    fetchProfiles: state.fetchProfiles, fetchStatus: state.fetchStatus,
    refresh: state.refresh, lastError: state.lastError, clearError: state.clearError,
  })));

  useEffect(() => {
    fetchProfiles();
    fetchStatus();
  }, [fetchProfiles, fetchStatus]);

  // 状态栏切换 profile 后，后端 emit "profile-switched"，刷新当前状态与列表
  useEffect(() => {
    if (!isTauri()) return;
    const unlistenPromise = listen('profile-switched', () => {
      void refresh();
    });
    void unlistenPromise.catch((error) => console.error('Profile event listener failed:', error));
    return () => {
      void unlistenPromise.then((unlisten) => unlisten()).catch(() => {});
    };
  }, [refresh]);

  return (
    <HashRouter>
      <div className="app-bg w-full h-full flex text-ink">
        <Sidebar />
        <main className="flex-1 min-w-0 overflow-y-auto overflow-x-hidden">
          {lastError && (
            <div role="alert" className="sticky top-0 z-20 border-b border-danger/30 bg-card px-4 py-2 text-[12.5px] text-danger sm:px-7">
              <div className="flex items-start justify-between gap-3">
                <span className="min-w-0 break-words">{lastError}</span>
                <button type="button" title="关闭错误提示" aria-label="关闭错误提示" className="icon-button" onClick={clearError}><X size={16} /></button>
              </div>
            </div>
          )}
          <Suspense fallback={<div role="status" aria-label="加载页面" className="grid place-items-center py-24"><Spinner size="lg" /></div>}>
            <Routes>
            <Route path="/" element={<Navigate to="/profiles" replace />} />
            <Route path="/profiles" element={<ProfilesPage />} />
            <Route path="/config" element={<ConfigPage />} />
            <Route path="/status" element={<StatusPage />} />
            <Route path="/import" element={<ImportPage />} />
            <Route path="/export" element={<ExportPage />} />
            <Route path="/history" element={<HistoryPage />} />
            <Route path="*" element={<Navigate to="/profiles" replace />} />
            </Routes>
          </Suspense>
        </main>
      </div>
    </HashRouter>
  );
}

export default App;
