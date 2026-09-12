import { beforeEach, describe, expect, it, vi } from 'vitest';
import { readSelectedTool, readSwitchProbe, writeSelectedTool, writeSwitchProbe } from './settings';

function installMemoryStorage() {
  const data = new Map<string, string>();
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => (data.has(k) ? data.get(k)! : null),
    setItem: (k: string, v: string) => { data.set(k, String(v)); },
    removeItem: (k: string) => { data.delete(k); },
    clear: () => { data.clear(); },
  } as Storage);
}

beforeEach(() => {
  vi.unstubAllGlobals();
});

describe('settings', () => {
  it('round-trips tool and probe prefs, rejects unknown tools', () => {
    installMemoryStorage();
    expect(readSelectedTool()).toBeNull();
    expect(readSwitchProbe()).toBe(false);
    writeSelectedTool('codex');
    writeSwitchProbe(true);
    expect(readSelectedTool()).toBe('codex');
    expect(readSwitchProbe()).toBe(true);
    localStorage.setItem('helio-tool', 'nope');
    expect(readSelectedTool()).toBeNull();
  });

  it('degrades gracefully without localStorage', () => {
    expect(readSelectedTool()).toBeNull();
    expect(readSwitchProbe()).toBe(false);
    expect(() => writeSelectedTool('codex')).not.toThrow();
    expect(() => writeSwitchProbe(true)).not.toThrow();
  });
});
