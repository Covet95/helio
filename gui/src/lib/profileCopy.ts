import type { ApiProfile } from '../types';

export function profileApiUrlText(profile: ApiProfile): string {
  return profile.api_url;
}

export function profileApiKeyText(profile: ApiProfile): string {
  return profile.api_key;
}
