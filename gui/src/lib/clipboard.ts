import { tauriApi } from './tauri';

type NativeCopy = (text: string) => Promise<void>;

export async function copyText(text: string, nativeCopy: NativeCopy = tauriApi.copyText): Promise<void> {
  await nativeCopy(text);
}
