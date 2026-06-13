import { BrowserRouter, Routes, Route, Navigate } from 'react-router-dom';
import { useEffect } from 'react';
import { useStore } from './store';
import Sidebar from './components/layout/Sidebar';
import ProfilesPage from './pages/ProfilesPage';
import ConfigPage from './pages/ConfigPage';
import StatusPage from './pages/StatusPage';
import ExportPage from './pages/ExportPage';

function App() {
  const { fetchProfiles, fetchStatus } = useStore();

  useEffect(() => {
    fetchProfiles();
    fetchStatus();
  }, [fetchProfiles, fetchStatus]);

  return (
    <BrowserRouter>
      <div className="app-bg w-full h-full flex text-ink">
        <Sidebar />
        <main className="flex-1 overflow-y-auto overflow-x-hidden">
          <Routes>
            <Route path="/" element={<Navigate to="/profiles" replace />} />
            <Route path="/profiles" element={<ProfilesPage />} />
            <Route path="/config" element={<ConfigPage />} />
            <Route path="/status" element={<StatusPage />} />
            <Route path="/export" element={<ExportPage />} />
          </Routes>
        </main>
      </div>
    </BrowserRouter>
  );
}

export default App;
