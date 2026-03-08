import { createInterface } from 'node:readline';

const rl = createInterface({ input: process.stdin });

const sink = {
  sinkName: 'test-sink',
  monitorSource: 'test-sink.monitor',
  nodeId: 999,
};

const nodes = [
  {
    id: 1,
    displayName: 'Firefox',
    applicationName: 'Firefox',
    nodeName: 'node.firefox',
    description: 'Firefox',
    mediaName: 'Playback',
    binary: 'firefox',
    isDevice: false,
  },
  {
    id: 2,
    displayName: 'Speakers',
    applicationName: 'WirePlumber',
    nodeName: 'alsa_output.speakers',
    description: 'Speakers',
    mediaName: null,
    binary: 'wireplumber',
    isDevice: true,
  },
];

function send(message) {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

rl.on('line', (line) => {
  if (!line.trim()) {
    return;
  }

  let request;

  try {
    request = JSON.parse(line);
  } catch {
    send({ id: 0, error: 'invalid request' });
    return;
  }

  switch (request.method) {
    case 'hasPipeWire':
      send({ id: request.id, result: true });
      break;

    case 'listShareableNodes':
      send({
        id: request.id,
        result: request.includeDevices
          ? nodes
          : nodes.filter((node) => !node.isDevice),
      });
      break;

    case 'ensureVirtualSink':
      send({ id: request.id, result: sink });
      break;

    case 'routeNodes':
      if (!Array.isArray(request.nodeIds) || request.nodeIds.length === 0) {
        send({ id: request.id, error: 'at least one node id is required' });
      } else {
        send({ id: request.id, result: sink });
      }
      break;

    case 'clearRoutes':
      send({ id: request.id, result: null });
      break;

    case 'dispose':
      process.stdout.write(
        `${JSON.stringify({ id: request.id, result: null })}\n`,
        () => process.exit(0),
      );
      break;

    default:
      send({
        id: typeof request.id === 'number' ? request.id : 0,
        error: `unknown method: ${request.method}`,
      });
      break;
  }
});
