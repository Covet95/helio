import type { ApiProfile } from '../types';

export function profileApiCredentialsText(profile: ApiProfile): string {
  return `API URL: ${profile.api_url}\nAPI Key: ${profile.api_key}`;
}
