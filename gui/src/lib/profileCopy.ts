import type { ApiProfile } from '../types';
import { nextCopyName } from './profileNames';

type CopyableProfileConfig = Pick<
  ApiProfile,
  | 'name'
  | 'provider'
  | 'api_url'
  | 'api_key'
  | 'model_mapping'
  | 'model'
  | 'models'
  | 'reasoning_effort'
  | 'context_1m'
  | 'target_app'
>;

export function duplicateProfileDraft(
  profile: ApiProfile,
  existingNames: Iterable<string>,
): ApiProfile {
  return {
    ...profile,
    id: undefined,
    name: nextCopyName(profile.name, existingNames),
    created_at: undefined,
    updated_at: undefined,
  };
}

export function profileApiUrlText(profile: ApiProfile): string {
  return profile.api_url;
}

export function profileApiConfigText(profile: ApiProfile): string {
  const config: CopyableProfileConfig = {
    name: profile.name,
    provider: profile.provider,
    api_url: profile.api_url,
    api_key: profile.api_key,
    target_app: profile.target_app,
  };

  if (profile.model_mapping && Object.keys(profile.model_mapping).length > 0) {
    config.model_mapping = profile.model_mapping;
  }
  if (profile.model) {
    config.model = profile.model;
  }
  if (profile.models && profile.models.length > 0) {
    config.models = profile.models;
  }
  if (profile.reasoning_effort) {
    config.reasoning_effort = profile.reasoning_effort;
  }
  if (profile.context_1m !== undefined) {
    config.context_1m = profile.context_1m;
  }

  return JSON.stringify(config, null, 2);
}
