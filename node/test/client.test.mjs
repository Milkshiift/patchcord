import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import { AudioSharePatchbay, hasPipeWire } from '../patchcord.js';

const testDir = path.dirname(fileURLToPath(import.meta.url));
const helperScript = path.join(testDir, 'mock-helper.mjs');

function createPatchbay() {
  return new AudioSharePatchbay({
    command: process.execPath,
    args: [helperScript],
    sinkPrefix: 'custom-app-share',
    sinkDescription: 'Custom App Screen Share',
  });
}

test('hasPipeWire convenience function works', async () => {
  const result = await hasPipeWire({
    command: process.execPath,
    args: [helperScript],
  });

  assert.equal(result, true);
});

test('client proxies helper responses and applies custom configuration', async () => {
  const patchbay = createPatchbay();

  try {
    assert.equal(await patchbay.hasPipeWire(), true);

    const appNodes = await patchbay.listShareableNodes(false);
    assert.equal(appNodes.length, 1);
    assert.equal(appNodes[0].displayName, 'Firefox');
    assert.equal(appNodes[0].isDevice, false);

    const allNodes = await patchbay.listShareableNodes(true);
    assert.equal(allNodes.length, 2);

    const ensured = await patchbay.ensureVirtualSink();

    assert.deepEqual(ensured, {
      sinkName: 'custom-app-share',
      monitorSource: 'custom-app-share.monitor',
      nodeId: 999,
    });

    const routed = await patchbay.routeNodes([1]);
    assert.deepEqual(routed, ensured);

    await patchbay.clearRoutes();
  } finally {
    await patchbay.dispose();
  }
});

test('helper errors reject requests', async () => {
  const patchbay = createPatchbay();

  try {
    await assert.rejects(
        patchbay.routeNodes([]),
        /at least one node id is required/,
    );
  } finally {
    await patchbay.dispose();
  }
});

test('dispose is idempotent', async () => {
  const patchbay = createPatchbay();

  await patchbay.dispose();
  await patchbay.dispose();
});