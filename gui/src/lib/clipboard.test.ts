import { copyText } from './clipboard';

function equal(actual: unknown, expected: unknown) {
  if (actual !== expected) {
    throw new Error(`expected ${String(expected)}, got ${String(actual)}`);
  }
}

let copied = '';

await copyText('native clipboard text', async (text) => {
  copied = text;
});

equal(copied, 'native clipboard text');
