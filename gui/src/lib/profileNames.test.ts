import { nextCopyName } from './profileNames';

function equal(actual: string, expected: string) {
  if (actual !== expected) {
    throw new Error(`expected ${expected}, got ${actual}`);
  }
}

equal(nextCopyName('prod', []), 'prod-copy');
equal(nextCopyName('prod', ['prod-copy']), 'prod-copy-2');
equal(nextCopyName('prod', ['prod-copy', 'prod-copy-2']), 'prod-copy-3');
equal(nextCopyName('prod', ['PROD-COPY']), 'prod-copy-2');
