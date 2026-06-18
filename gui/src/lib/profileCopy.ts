import type { ApiProfile } from '../types';
import { nextCopyName } from './profileNames';

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
