import { useEffect } from 'react';
import { useStore } from './store';

function App() {
  const { fetchProfiles, fetchStatus, profiles, status } = useStore();

  useEffect(() => {
    fetchProfiles();
    fetchStatus();
  }, [fetchProfiles, fetchStatus]);

  return (
    <div className="w-full h-full bg-gray-50 flex items-center justify-center">
      <div className="text-center">
        <h1 className="text-3xl font-bold text-gray-900 mb-4">
          Switch API GUI
        </h1>
        <p className="text-gray-600 mb-2">
          Profiles: {profiles.length}
        </p>
        <p className="text-gray-600">
          Database: {status?.database?.profile_count || 0} profiles
        </p>
      </div>
    </div>
  );
}

export default App;
