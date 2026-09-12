/** 本地偏好读写：localStorage 键集中在一处，避免散落在组件里。 */
import { SUPPORTED_TOOLS, type TargetApp } from '@/types';

const TOOL_KEY = 'helio-tool';
const SWITCH_PROBE_KEY = 'helio-switch-probe';

const TOOL_IDS: ReadonlySet<string> = new Set(SUPPORTED_TOOLS.map((t) => t.id));

function read(key: string): string | null {
  try {
    if (typeof localStorage === 'undefined') return null;
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* 忽略持久化失败 */
  }
}

export function readSelectedTool(): TargetApp | null {
  const saved = read(TOOL_KEY);
  if (saved && TOOL_IDS.has(saved)) return saved as TargetApp;
  return null;
}

export function writeSelectedTool(tool: TargetApp): void {
  write(TOOL_KEY, tool);
}

export function readSwitchProbe(): boolean {
  return read(SWITCH_PROBE_KEY) === '1';
}

export function writeSwitchProbe(on: boolean): void {
  write(SWITCH_PROBE_KEY, on ? '1' : '0');
}
