import { useState } from 'react';
import { useStore } from '../store';
import { Button } from '../components/common/Button';
import { Spinner } from '../components/common/Spinner';
import { Database, Plus, Edit2, Trash2 } from 'lucide-react';
import type { ApiProfile, TargetApp } from '../types';
import { SUPPORTED_TOOLS } from '../types';

export default function ProfilesPage() {
  const { profiles, loadingProfiles, addProfile, updateProfile, deleteProfile, switchProfile } = useStore();
  const [showModal, setShowModal] = useState(false);
  const [editingProfile, setEditingProfile] = useState<ApiProfile | null>(null);

  const handleAdd = () => {
    setEditingProfile(null);
    setShowModal(true);
  };

  const handleEdit = (profile: ApiProfile) => {
    setEditingProfile(profile);
    setShowModal(true);
  };

  const handleDelete = async (name: string) => {
    if (confirm(`确定要删除 Profile "${name}" 吗？`)) {
      await deleteProfile(name);
    }
  };

  const handleSwitch = async (targetApp: TargetApp, profileName: string) => {
    await switchProfile(targetApp, profileName);
  };

  if (loadingProfiles) {
    return (
      <div className="flex items-center justify-center h-full">
        <Spinner size="lg" />
      </div>
    );
  }

  return (
    <div className="p-8">
      <div className="flex justify-between items-center mb-6">
        <h2 className="text-2xl font-bold text-gray-900">API Profiles</h2>
        <Button onClick={handleAdd} className="gap-2">
          <Plus size={18} />
          添加 Profile
        </Button>
      </div>

      {profiles.length === 0 ? (
        <div className="text-center py-12 text-gray-500">
          <Database size={48} className="mx-auto mb-4 opacity-50" />
          <p>还没有任何 Profile</p>
          <p className="text-sm mt-2">点击"添加 Profile"开始</p>
        </div>
      ) : (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
          {profiles.map((profile) => (
            <ProfileCard
              key={profile.name}
              profile={profile}
              onEdit={() => handleEdit(profile)}
              onDelete={() => handleDelete(profile.name)}
              onSwitch={handleSwitch}
            />
          ))}
        </div>
      )}

      {showModal && (
        <ProfileModal
          profile={editingProfile}
          onClose={() => setShowModal(false)}
          onSave={async (profile) => {
            if (editingProfile) {
              await updateProfile(profile);
            } else {
              await addProfile(profile);
            }
            setShowModal(false);
          }}
        />
      )}
    </div>
  );
}

function ProfileCard({
  profile,
  onEdit,
  onDelete,
  onSwitch,
}: {
  profile: ApiProfile;
  onEdit: () => void;
  onDelete: () => void;
  onSwitch: (app: TargetApp, name: string) => void;
}) {
  return (
    <div className="bg-white rounded-lg shadow p-4 hover:shadow-md transition-shadow">
      <div className="flex justify-between items-start mb-3">
        <div>
          <h3 className="font-semibold text-lg">{profile.name}</h3>
          <p className="text-sm text-gray-500">{profile.provider}</p>
        </div>
        <div className="flex gap-2">
          <button
            onClick={onEdit}
            className="p-1 text-gray-400 hover:text-primary"
            title="编辑"
          >
            <Edit2 size={16} />
          </button>
          <button
            onClick={onDelete}
            className="p-1 text-gray-400 hover:text-error"
            title="删除"
          >
            <Trash2 size={16} />
          </button>
        </div>
      </div>

      <div className="text-sm text-gray-600 mb-3">
        <p className="truncate">URL: {profile.api_url}</p>
        <p className="truncate">Key: {profile.api_key.slice(0, 10)}...</p>
      </div>

      <div className="space-y-2">
        <p className="text-xs text-gray-400 font-medium">切换到：</p>
        <div className="grid grid-cols-2 gap-2">
          {SUPPORTED_TOOLS.map((tool) => (
            <Button
              key={tool.id}
              size="sm"
              variant="secondary"
              onClick={() => onSwitch(tool.id, profile.name)}
            >
              {tool.displayName}
            </Button>
          ))}
        </div>
      </div>
    </div>
  );
}

function ProfileModal({
  profile,
  onClose,
  onSave,
}: {
  profile: ApiProfile | null;
  onClose: () => void;
  onSave: (profile: ApiProfile) => void;
}) {
  const [formData, setFormData] = useState<ApiProfile>(
    profile || {
      name: '',
      provider: 'Anthropic',
      api_url: 'https://api.anthropic.com',
      api_key: '',
    }
  );

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    onSave(formData);
  };

  return (
    <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50">
      <div className="bg-white rounded-lg p-6 w-full max-w-md">
        <h3 className="text-xl font-bold mb-4">
          {profile ? '编辑 Profile' : '添加 Profile'}
        </h3>

        <form onSubmit={handleSubmit} className="space-y-4">
          <div>
            <label className="block text-sm font-medium mb-1">名称</label>
            <input
              type="text"
              value={formData.name}
              onChange={(e) => setFormData({ ...formData, name: e.target.value })}
              className="w-full px-3 py-2 border rounded-lg"
              required
              disabled={!!profile}
            />
          </div>

          <div>
            <label className="block text-sm font-medium mb-1">Provider</label>
            <input
              type="text"
              value={formData.provider}
              onChange={(e) => setFormData({ ...formData, provider: e.target.value })}
              className="w-full px-3 py-2 border rounded-lg"
              required
            />
          </div>

          <div>
            <label className="block text-sm font-medium mb-1">API URL</label>
            <input
              type="url"
              value={formData.api_url}
              onChange={(e) => setFormData({ ...formData, api_url: e.target.value })}
              className="w-full px-3 py-2 border rounded-lg"
              required
            />
          </div>

          <div>
            <label className="block text-sm font-medium mb-1">API Key</label>
            <input
              type="password"
              value={formData.api_key}
              onChange={(e) => setFormData({ ...formData, api_key: e.target.value })}
              className="w-full px-3 py-2 border rounded-lg"
              required
            />
          </div>

          <div className="flex gap-2 justify-end mt-6">
            <Button type="button" variant="secondary" onClick={onClose}>
              取消
            </Button>
            <Button type="submit">
              {profile ? '保存' : '添加'}
            </Button>
          </div>
        </form>
      </div>
    </div>
  );
}
